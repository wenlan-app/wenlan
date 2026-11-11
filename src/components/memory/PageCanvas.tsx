// SPDX-License-Identifier: AGPL-3.0-only
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  useId,
  type Dispatch,
  type KeyboardEvent as ReactKeyboardEvent,
  type RefObject,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type SetStateAction,
} from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  Background,
  ControlButton,
  Controls,
  ReactFlow,
  ReactFlowProvider,
  useNodesState,
  useReactFlow,
  type Edge,
  type FinalConnectionState,
  type Node,
  type EdgeTypes,
  type NodeTypes,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import {
  asPageMapApiError,
  createPageMapNode,
  deletePageMapNode,
  getPage,
  getPageMap,
  improvePageMap,
  patchPageMapNode,
  putPageMapLayout,
  updatePage,
  type PageMapLayoutPosition,
  type PageMap,
  type PageMapNode,
  type PageMapStatus,
} from "../../lib/tauri";
import {
  layoutIndented,
  layoutMap,
  layoutTree,
  lineBoxWidth,
  nodeBoxSize,
  NODE_HEIGHT,
  OUTLINE_NODE_HEIGHT,
} from "../../lib/pageMap/tree";
import {
  branchHeadings,
  branchSlots,
  foldableHeadings,
  isPageLine,
  lineAncestors,
  lineDepths,
  lineIdOf,
  moreFor,
  outlineLine,
  PAGE_LINE_PREFIX,
  PAGE_LINE_ROOT,
  shutByDepth,
  withinLine,
  withPageLines,
} from "../../lib/pageMap/pageLines";
import { mixOver, useGraphPalette } from "../../lib/graph/palette";
import { applyOutlineEdit, editableText, type MovePlace, type OutlineEdit } from "../../lib/pageMap/outlineEdits";
import CanvasNode, { type CanvasNodeData } from "./canvas/CanvasNode";
import SpineEdge from "./canvas/SpineEdge";
import "./canvas/pageCanvasApply.css";

interface PageCanvasProps {
  pageId: string;
  pageTitle: string;
  /** "{ref_kind}:{ref_id}" -> display name, for nodes the daemon left label:null */
  labelOverrides: ReadonlyMap<string, string>;
  onMemoryClick: (sourceId: string) => void;
  onPageClick?: (pageId: string) => void;
  onEntityClick?: (entityId: string) => void;
  /** Show the part of the page a heading or item of the map came from. */
  onSectionClick?: (line: { text: string; heading: string | null }) => void;
  /** The page has edits in its editor not yet saved: its lines wait until they are. */
  pageEditDirty?: boolean;
}

/** Why an edit made on the map did not reach the page. */
class LineEditRefused extends Error {
  constructor(readonly reason: "moved" | "notEditable" | "failed") {
    super(`page line edit refused: ${reason}`);
    this.name = "LineEditRefused";
  }
}

/** Inline marks a renamed line would lose, since the box shows plain text. */
const INLINE_MARKS = /\[\[|\]\(|`|<[a-z]/i;

// The map shape this build knows how to edit. A map stamped higher was written
// by a newer Wenlan: render it, never write to it (spec: degrade, never
// corrupt).
const SUPPORTED_MAP_SCHEMA = 2;
const LAYOUT_DEBOUNCE_MS = 600;

function supportsIndependentIdeas(map: PageMap): boolean {
  return map.map_schema >= 1 &&
    map.map_schema <= SUPPORTED_MAP_SCHEMA &&
    map.independent_ideas === true;
}

// A box the user is still naming. It lives only in local state and never
// reaches the daemon: a node's `ref_id` is fixed at creation (PATCH takes
// label/pinned/status/rank/parent_id and nothing else), so the name has to be
// settled *before* the create call rather than patched in afterwards.
const DRAFT_ID = "__draft__";
const DRAFT_SIZE = { width: 170, height: NODE_HEIGHT };
// Where a new box lands relative to its parent, until the next layout pass
// gives it a real radial slot.
const DRAFT_OFFSET = { x: 150, y: 64 };

// Key caps stay expressions rather than literal JSX text: they name physical
// keys, not copy, so they are not translated and must not read as new
// hardcoded strings.
// `as const` is load-bearing: i18n keys are a literal union here, and a
// `string` annotation would widen them past the point where t() type-checks.
// How far an arrow key moves the selection, and how far with Shift held.
const NUDGE_STEP = 8;
const NUDGE_STEP_COARSE = 40;

// The shortcut list behind the corner disclosure. Gestures lead because they are
// what someone tries in the first ten seconds; the key caps behind them are the
// same actions, faster.
// `as const` is load-bearing: i18n keys are a literal union here, and a
// `string` annotation would widen them past the point where t() type-checks.
const SHORTCUTS = [
  { cap: "N", key: "pageCanvas.hintCreate" },
  { cap: "Right-click", key: "pageCanvas.hintMenu" },
  { cap: "Shift F10", key: "pageCanvas.hintMenu" },
  { cap: "Double-click", key: "pageCanvas.hintCreate" },
  { cap: "Drag", key: "pageCanvas.hintMarquee" },
  { cap: "Tab", key: "pageCanvas.hintAddChild" },
  { cap: "Enter", key: "pageCanvas.hintAddSibling" },
  { cap: "F2", key: "pageCanvas.hintRename" },
  { cap: "Delete", key: "pageCanvas.hintDelete" },
  { cap: "Option ↑ ↓", key: "pageCanvas.hintMoveLine" },
  { cap: "Option 1–9, 0", key: "pageCanvas.hintFoldLevel" },
  { cap: "/", key: "pageCanvas.hintSearch" },
  { cap: "+ -", key: "pageCanvas.hintZoom" },
  { cap: "Shift 0", key: "pageCanvas.hintActualSize" },
  { cap: "Shift 1", key: "pageCanvas.hintFit" },
  { cap: "Shift 2", key: "pageCanvas.hintZoomSelection" },
  { cap: "Shift /", key: "pageCanvas.hintHelp" },
] as const;


/**
 * How the map is framed when it opens.
 *
 * A bare `fitView` frames the extent and nothing else, which on a map this
 * shape is the wrong thing to optimise: a four-box page opened at 1.167, so
 * every box rendered larger than it was designed, and a thirteen-box page
 * opened at 0.563, which rasterizes a 12px label at under 7px. Legibility is
 * the constraint, extent is not — so the opening scale is clamped to a band
 * either side of actual size, and a map too big for the band opens readable and
 * partly off-frame. Seeing all of it is one key or one button away; reading it
 * is what the reader came for.
 *
 * The floor is derived rather than chosen: 12px labels at 0.75 render at 9px,
 * which is the smallest defensible size for UI text. It governs the cold open
 * and nothing else — "fit all" has no floor at all, by design.
 */
const OPENING_VIEW = { minZoom: 0.75, maxZoom: 1, padding: 0.15 } as const;

/**
 * How the map is framed by "fit all" — the key, the button, and the menu item,
 * which are one command and have to agree.
 *
 * They did not: each reached React Flow's bare defaults separately, so opening a
 * thirteen-box page at 0.812 and then pressing the fit key moved it to 0.846 —
 * a change with no cause a reader can see, which is exactly the kind of thing
 * that makes a canvas feel like it needs watching. Same padding as the opening
 * view fixes that.
 *
 * The ceiling is shared too, for a different reason: fitting a four-box page to
 * the frame means magnifying it to 1.167, drawing every box half again larger
 * than it was designed. Actual size is as large as anything should ever get.
 *
 * No floor, deliberately. A command that says it shows everything has to show
 * everything, however small it ends up.
 */
const FIT_ALL_VIEW = { maxZoom: 1, padding: 0.15, duration: 200 } as const;

// How much a key press zooms, and how long the move takes. React Flow's own
// step is 1.2x, which the Controls buttons use; matching it keeps the button
// and the key the same gesture.
const ZOOM_DURATION = 180;

/**
 * How a line arriving at a box on ring `depth` is drawn.
 *
 * A gentle arc along the spoke, drawn by `SpineEdge` — see that file for why
 * neither of React Flow's own edge types will do it. The short version: the
 * default bezier leaves from the bottom and arrives at the top whatever
 * direction the child actually lies in, which turns every branch running
 * sideways or upward into an S, and `straight` drew the map as a wire diagram.
 *
 * The two inks are the canvas's own (`--canvas-edge*`), not the graph palette's:
 * the knowledge-graph views composite their edges over a dark ground where a
 * fainter stroke still reads, and at this size those values sank into the page
 * surface. Named in CSS rather than resolved here so both themes stay in one
 * place — `var()` is legal in an SVG `stroke`.
 */
function spineLook(depth: number): Partial<Edge> {
  return {
    type: "spine",
    style:
      depth <= 1
        ? { stroke: "var(--canvas-edge-strong)", strokeWidth: 1.6 }
        : { stroke: "var(--canvas-edge)", strokeWidth: 1.1 },
  };
}

/**
 * A spoke inside a page heading's branch takes that branch's ink, softened
 * toward the surface, and goes quiet with its line when another branch is in
 * focus. Spokes with no branch (the page's lead lines, stored boxes) keep the
 * canvas inks.
 */
function branchLook(
  base: Partial<Edge>,
  ink: string | undefined,
  depth: number,
  dimmed: boolean,
  surface: string,
): Partial<Edge> {
  const style = { ...base.style };
  if (ink) {
    style.stroke = mixOver(ink, surface, depth <= 1 ? 0.75 : 0.55);
    style.strokeWidth = depth <= 1 ? 2 : depth === 2 ? 1.4 : 1;
  }
  if (dimmed) style.opacity = 0.25;
  return { ...base, style };
}

/**
 * A guess the map would show once revealed. Stored section boxes give way to
 * the page's own headings whenever the daemon sends them, so a guessed section
 * then has nowhere to appear and must not be counted.
 */
function countsAsSuggestion(n: PageMapNode, map: PageMap | undefined): boolean {
  if (n.status !== "suggested") return false;
  return !(n.ref_kind === "section" && (map?.outline?.length ?? 0) > 0);
}

/** Drawn as text on a stroke rather than as a box: page lines past the first ring. */
function onStroke(v: { node: { id: string }; depth: number }): boolean {
  return isPageLine(v.node.id) && v.depth >= 2 && moreFor(v.node.id) === null;
}

type Placed = { node: { id: string }; depth: number; x: number; y: number; width: number; height: number };

/**
 * Where a left-to-right tree line meets its two boxes. A filled box hides the
 * part of a center-to-center line inside it; text on a stroke hides nothing, so
 * the line leaves from the end of the parent's stroke and runs into the start
 * of the child's.
 */
function treeEnds(from: Placed, to: Placed) {
  const fromOnStroke = onStroke(from);
  const toOnStroke = onStroke(to);
  return {
    fromX: from.x + (fromOnStroke ? from.width / 2 : 0),
    fromY: from.y + (fromOnStroke ? from.height / 2 : 0),
    toX: to.x - (toOnStroke || moreFor(to.node.id) !== null ? to.width / 2 : 0),
    toY: to.y + (toOnStroke ? to.height / 2 : 0),
  };
}

/** An open right-click menu, positioned in surface-local pixels. */
type CanvasMenu =
  | { kind: "pane"; x: number; y: number; clientX: number; clientY: number }
  | { kind: "node"; x: number; y: number; nodeId: string };

interface MenuItem {
  key: string;
  label: string;
  run: () => void | boolean;
  danger?: boolean;
  disabled?: boolean;
}

/**
 * A subtree delete that stopped partway. The daemon tombstones one node per
 * call and every tombstone is terminal, so a failure on box four of seven is
 * not a no-op the user can retry — three boxes are already gone for good.
 * Carries the count so the notice can say so.
 */
class PartialSubtreeDelete extends Error {
  constructor(
    readonly reason: unknown,
    readonly label: string,
    readonly deleted: number,
    readonly total: number,
  ) {
    super("page map subtree delete stopped partway");
    this.name = "PartialSubtreeDelete";
  }
}

function readOpenHeadings(key: string): Set<string> {
  try {
    const raw = localStorage.getItem(key);
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return new Set(Array.isArray(parsed) ? parsed.filter((v): v is string => typeof v === "string") : []);
  } catch {
    return new Set();
  }
}

/** Keep which headings are open or shut on this device; a convenience only. */
function rememberHeadings(key: string, ids: ReadonlySet<string>): void {
  try {
    localStorage.setItem(key, JSON.stringify([...ids]));
  } catch {
    // Private windows and blocked storage: the fold still works this session.
  }
}

const nodeTypes: NodeTypes = { pageMapNode: CanvasNode as NodeTypes[string] };
const edgeTypes: EdgeTypes = { spine: SpineEdge };

export default function PageCanvas(props: PageCanvasProps) {
  // useReactFlow (for the viewport we persist) only works under a provider.
  return (
    <ReactFlowProvider>
      <PageCanvasInner {...props} />
    </ReactFlowProvider>
  );
}

function PageCanvasInner({
  pageId,
  pageTitle,
  labelOverrides,
  onMemoryClick,
  onPageClick,
  onEntityClick,
  onSectionClick,
  pageEditDirty = false,
}: PageCanvasProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const palette = useGraphPalette();
  const {
    getViewport,
    setViewport,
    screenToFlowPosition,
    fitView,
    zoomIn,
    zoomOut,
    zoomTo,
    setCenter,
    getZoom,
    viewportInitialized,
  } = useReactFlow();
  const [notice, setNotice] = useState<string | null>(null);
  const [menu, setMenu] = useState<CanvasMenu | null>(null);
  const surfaceRef = useRef<HTMLDivElement | null>(null);
  const menuElementRef = useRef<HTMLUListElement | null>(null);
  const menuReturnFocusRef = useRef<HTMLElement | null>(null);
  // Node state is local and React Flow owns it during a drag. This is what
  // makes dragging cheap: a pointer move rewrites one node's position instead
  // of re-deriving the whole tree, and every other node keeps its identity so
  // memo(CanvasNode) actually holds.
  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [draft, setDraft] = useState<{
    parentId: string;
    x: number;
    y: number;
    label: string;
    refId: string;
    /** Set when the new box is a new line of the page rather than a box of the map's own. */
    line?: { op: "addUnder" | "addAfter"; line: number };
  } | null>(null);
  // The named idea stays visible while the map write/refetch completes. Its
  // ref_id is stable across retries so an uncertain response cannot duplicate it.
  const [pending, setPending] = useState<
    { parentId: string; label: string; x: number; y: number; refId: string; line?: { op: "addUnder" | "addAfter"; line: number } } | null
  >(null);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [showSuggestions, setShowSuggestions] = useState(false);
  // Headings the person unfolded on this page, remembered per page on this
  // device. A convenience only: losing it just folds long sections again.
  const openKey = `wenlan.pageMap.open.${pageId}`;
  const [openHeadings, setOpenHeadings] = useState<Set<string>>(() => readOpenHeadings(openKey));
  useEffect(() => setOpenHeadings(readOpenHeadings(openKey)), [openKey]);
  // A heading the person clicked: its branch stays bright, the rest goes quiet
  // but stays in view. Escape or a click on empty canvas lets go.
  const [focusLine, setFocusLine] = useState<string | null>(null);
  useEffect(() => setFocusLine(null), [pageId]);
  const toggleHeading = useCallback((id: string) => {
    setOpenHeadings((current) => {
      const next = new Set(current);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      try {
        localStorage.setItem(openKey, JSON.stringify([...next]));
      } catch {
        // Private windows and blocked storage: the fold still works this session.
      }
      return next;
    });
  }, [openKey]);
  // Headings the person shut with their arrow, remembered the same way.
  const closedKey = `wenlan.pageMap.closed.${pageId}`;
  const [closedHeadings, setClosedHeadings] = useState<Set<string>>(() => readOpenHeadings(closedKey));
  useEffect(() => setClosedHeadings(readOpenHeadings(closedKey)), [closedKey]);
  const [helpOpen, setHelpOpen] = useState(false);
  const [ideaDraft, setIdeaDraft] = useState("");
  const [ideaEntryOpen, setIdeaEntryOpen] = useState(false);
  const ideaDraftRefId = useRef<string | null>(null);
  const ideaSubmissionAttemptedRef = useRef(false);
  const refocusSurfaceRef = useRef(false);
  const [ideaError, setIdeaError] = useState<string | null>(null);
  const ideaNameInputId = useId();
  const ideaEntryRef = useRef<HTMLFormElement | null>(null);
  const [ideaEntryAnchor, setIdeaEntryAnchor] = useState<{ x: number; y: number } | null>(null);
  const activePageRef = useRef({ pageId, pageTitle });
  activePageRef.current = { pageId, pageTitle };

  useLayoutEffect(() => {
    if (!ideaEntryOpen && refocusSurfaceRef.current) {
      refocusSurfaceRef.current = false;
      surfaceRef.current?.focus({ preventScroll: true });
    }
  }, [ideaEntryOpen]);

  const {
    data: map,
    isLoading,
    error,
    refetch,
  } = useQuery({
    queryKey: ["page-map", pageId],
    queryFn: () => getPageMap(pageId),
    retry: false,
  });

  const readOnly = !!map && map.map_schema > SUPPORTED_MAP_SCHEMA;
  const independentIdeasSupported = !!map && supportsIndependentIdeas(map);

  const invalidate = useCallback(
    () => queryClient.invalidateQueries({ queryKey: ["page-map", pageId] }),
    [queryClient, pageId],
  );

  // Nodes the user has moved whose positions the daemon has not stored yet. A
  // resync must not overwrite these or the box snaps back to its old slot
  // mid-burst, which reads as the drag being ignored.
  //
  // Stamped per drop rather than kept as a plain set: a drag that starts while
  // the previous PUT is still in flight would otherwise be cleared by that
  // PUT's completion, and the refetch behind it would yank the box back to the
  // position the user just moved it away from.
  const dirtyRef = useRef(new Map<string, number>());
  const dragSeq = useRef(0);
  const nodesRef = useRef<Node[]>([]);
  useEffect(() => {
    nodesRef.current = nodes;
  }, [nodes]);

  useEffect(() => {
    dirtyRef.current.clear();
    setDraft(null);
    setPending(null);
    setEditingId(null);
    setNotice(null);
    setMenu(null);
    setShowSuggestions(false);
    setIdeaDraft("");
    setIdeaEntryOpen(false);
    ideaDraftRefId.current = null;
    ideaSubmissionAttemptedRef.current = false;
    setIdeaError(null);
    setIdeaEntryAnchor(null);
  }, [pageId, pageTitle]);

  const untitled = t("pageCanvas.untitled");

  // Suggestions are the daemon's guesses, not the user's map, so a page opens
  // on what the user actually put there. The counted reveal keeps them reachable.
  //
  // The page's own headings and items join them read-only: the map opens on
  // what the page already says (UX-VIEW-4).
  const deepShut = useMemo(() => shutByDepth(map), [map]);
  // A heading is open when nothing it hangs on is shut: a deep one starts
  // shut until opened, every other one starts open until shut.
  const branchOpen = useCallback(
    (id: string) => !closedHeadings.has(id) && (!deepShut.has(id) || openHeadings.has(id)),
    [closedHeadings, deepShut, openHeadings],
  );
  const toggleBranch = useCallback(
    (id: string) => {
      const opening = !branchOpen(id);
      setClosedHeadings((current) => {
        const next = new Set(current);
        if (opening) next.delete(id);
        else next.add(id);
        try {
          localStorage.setItem(closedKey, JSON.stringify([...next]));
        } catch {
          // Blocked storage: the branch still opens and shuts this session.
        }
        return next;
      });
      if (deepShut.has(id) && opening !== openHeadings.has(id)) toggleHeading(id);
    },
    [branchOpen, closedKey, deepShut, openHeadings, toggleHeading],
  );
  // The whole map shut down to one level at once (Option 1–9), or opened
  // all the way (Option 0), the way an outliner's level keys work.
  const foldToLevel = useCallback(
    (level: number | null) => {
      const depth = lineDepths(map);
      const closed = new Set<string>();
      const open = new Set(openHeadings);
      for (const id of branchHeadings(map)) {
        if (level !== null && (depth.get(id) ?? 0) >= level) {
          closed.add(id);
          open.delete(id);
        } else if (deepShut.has(id)) open.add(id);
      }
      if (level === null) for (const id of foldableHeadings(map)) open.add(id);
      setClosedHeadings(closed);
      setOpenHeadings(open);
      rememberHeadings(closedKey, closed);
      rememberHeadings(openKey, open);
    },
    [map, openHeadings, deepShut, closedKey, openKey],
  );
  // A line found by search: everything it hangs on opens so it is drawn, then
  // the map moves to it once it is laid out.
  const [revealId, setRevealId] = useState<string | null>(null);
  const revealLine = useCallback(
    (lineId: string) => {
      const ups = lineAncestors(map, lineId);
      const parent = ups[0];
      setClosedHeadings((current) => {
        const next = new Set(current);
        for (const id of ups) next.delete(id);
        rememberHeadings(closedKey, next);
        return next;
      });
      setOpenHeadings((current) => {
        const next = new Set(current);
        for (const id of ups) if (deepShut.has(id) || id === parent) next.add(id);
        rememberHeadings(openKey, next);
        return next;
      });
      setRevealId(`${PAGE_LINE_PREFIX}${lineId}`);
    },
    [map, deepShut, closedKey, openKey],
  );
  const [search, setSearch] = useState<{ query: string; at: number } | null>(null);
  useEffect(() => setSearch(null), [pageId]);
  const searchMatches = useMemo(() => {
    const query = search?.query.trim().toLocaleLowerCase();
    if (!query) return [];
    return (map?.outline ?? [])
      .filter((line) => `${line.label} ${line.text}`.toLocaleLowerCase().includes(query))
      .map((line) => line.id);
  }, [search?.query, map]);
  const searchStep = useCallback(
    (direction: 1 | -1) => {
      if (!search || searchMatches.length === 0) return;
      const total = searchMatches.length;
      const at = search.at < 0 ? (direction === 1 ? 0 : total - 1) : (search.at + direction + total) % total;
      setSearch({ ...search, at });
      revealLine(searchMatches[at]);
    },
    [search, searchMatches, revealLine],
  );
  const showRestLabel = t("pageCanvas.showRest");
  const visibleNodes = useMemo(
    () =>
      withPageLines(map, {
        pageId,
        pageTitle,
        open: openHeadings,
        closed: closedHeadings,
        showSuggestions,
        showRestLabel,
      }),
    [map, pageId, pageTitle, openHeadings, closedHeadings, showSuggestions, showRestLabel],
  );
  // Every heading with lines under it carries an arrow to shut its branch.
  const foldable = useMemo(() => branchHeadings(map), [map]);
  const hasPageLines = useMemo(() => visibleNodes.some((n) => isPageLine(n.id)), [visibleNodes]);
  // The side panel is a column: the page's lines read top to bottom there,
  // indented by level. With room to spare they grow left to right.
  const [narrow, setNarrow] = useState(true);
  const layoutMode = !hasPageLines ? "radial" : narrow ? "indent" : "tree";
  const views = useMemo(() => {
    if (layoutMode === "radial") return layoutMap(visibleNodes, labelOverrides, untitled, "portrait");
    const heightOf = (n: PageMapNode) => (isPageLine(n.id) ? OUTLINE_NODE_HEIGHT : NODE_HEIGHT);
    const widthOf = (n: PageMapNode, label: string, base: number) =>
      isPageLine(n.id)
        ? lineBoxWidth(label, foldable.has(n.id.slice(PAGE_LINE_PREFIX.length)) ? 24 : 0)
        : base;
    return layoutMode === "indent"
      ? layoutIndented(visibleNodes, labelOverrides, untitled, heightOf, widthOf)
      : layoutTree(visibleNodes, labelOverrides, untitled, heightOf, widthOf);
  }, [layoutMode, visibleNodes, labelOverrides, untitled, foldable]);
  // Nothing stored yet. The page's lines may still be on screen; this only
  // decides how a new box of the person's own gets its first parent.
  const storedRootId = useMemo(
    () => map?.nodes.find((n) => n.parent_id === null && n.status !== "dismissed")?.id ?? null,
    [map?.nodes],
  );
  const isEmptyMap = !map || map.revision === 0 || storedRootId === null;

  // Suggestions outlive the session that hid them: `showSuggestions` resets on
  // every mount, so without a count to offer back, a reload would strand the
  // ones already in the map where nobody can accept or dismiss them.
  const hiddenSuggestions = useMemo(
    () =>
      showSuggestions
        ? 0
        : (map?.nodes ?? []).filter((n) => countsAsSuggestion(n, map)).length,
    [map, showSuggestions],
  );

  // Box metrics and collapse state by id, so the layout PUT can be built from
  // local positions without walking the tree again.
  const metaRef = useRef(new Map<string, { width: number; height: number; collapsed: boolean }>());
  useEffect(() => {
    metaRef.current = new Map(
      views.filter((v) => !isPageLine(v.node.id)).map((v) => [
        v.node.id,
        { width: v.width, height: v.height, collapsed: v.node.collapsed },
      ]),
    );
  }, [views]);

  const revisionRef = useRef(map?.revision ?? 0);
  useEffect(() => {
    revisionRef.current = map?.revision ?? 0;
  }, [map?.revision]);

  // A losing race is not an error the user caused: say what happened, refresh
  // to the server's truth, and let them redo the one action.
  const handleMutationError = useCallback(
    (e: unknown) => {
      if (asPageMapApiError(e)?.status === 409) {
        setNotice(t("pageCanvas.conflict"));
        void invalidate();
        void refetch();
        return;
      }
      setNotice(t("pageCanvas.mutationError"));
    },
    [invalidate, refetch, t],
  );

  const onMutated = useCallback(() => {
    setNotice(null);
    void invalidate();
  }, [invalidate]);

  const improveMutation = useMutation({
    mutationFn: async () => {
      if (!map || readOnly || map.map_schema < 1) {
        throw new Error("page map generation is unavailable");
      }
      return improvePageMap(pageId);
    },
    onMutate: () => setNotice(t("pageCanvas.generating")),
    onSuccess: (fresh) => {
      onMutated();
      const found = fresh.nodes.some((n) => n.status === "suggested" && n.ref_kind !== "section");
      setShowSuggestions(found);
      if (!found) setNotice(t("pageCanvas.noRelated"));
    },
    onError: handleMutationError,
  });

  // Each accept moves the map's revision, so they go one after another, each
  // on the revision the last one returned.
  const acceptAllMutation = useMutation({
    mutationFn: async () => {
      let revision = revisionRef.current;
      const ids = (map?.nodes ?? [])
        .filter((n) => n.status === "suggested" && n.ref_kind !== "section")
        .map((n) => n.id);
      for (const id of ids) {
        const result = await patchPageMapNode(pageId, id, { base_revision: revision, status: "active" });
        revision = result.revision;
      }
    },
    onSuccess: onMutated,
    onError: handleMutationError,
  });

  const statusMutation = useMutation({
    mutationFn: ({ nodeId, status }: { nodeId: string; status: PageMapStatus }) =>
      patchPageMapNode(pageId, nodeId, {
        base_revision: revisionRef.current,
        status,
      }),
    onSuccess: onMutated,
    onError: handleMutationError,
  });

  const renameMutation = useMutation({
    mutationFn: ({ nodeId, label }: { nodeId: string; label: string }) =>
      patchPageMapNode(pageId, nodeId, {
        base_revision: revisionRef.current,
        label,
      }),
    onSuccess: (_result, variables) => {
      setEditingId((current) => current === variables.nodeId ? null : current);
      onMutated();
    },
    onError: handleMutationError,
  });

  // The daemon's DELETE tombstones exactly one node — no cascade, no reparent —
  // and `buildSpine` drops any box whose parent is gone. Tombstoning a parent on
  // its own would therefore make its live children permanently invisible, so the
  // subtree comes down with it, deepest first.
  //
  // ponytail: sequential, because each tombstone bumps the map revision and the
  // DELETE response does not carry the new one. A leaf still costs exactly one
  // call; only a real subtree pays for the re-reads. Cascading daemon-side would
  // collapse this to a single request.
  const removeSubtree = useCallback(
    async (roots: string[]) => {
      const childrenOf = new Map<string, string[]>();
      for (const v of views) {
        const parent = v.node.parent_id;
        if (!parent) continue;
        const kids = childrenOf.get(parent);
        if (kids) kids.push(v.node.id);
        else childrenOf.set(parent, [v.node.id]);
      }
      const ordered: string[] = [];
      const seen = new Set<string>();
      const walk = (id: string) => {
        if (seen.has(id)) return;
        seen.add(id);
        for (const kid of childrenOf.get(id) ?? []) walk(kid);
        ordered.push(id); // post-order: every child precedes its parent
      };
      for (const id of roots) walk(id);

      // Named while `views` still describes it — after the refetch these nodes
      // are gone and there is nothing left to read a label off. Read before the
      // loop so a failure partway can still say what it was working on.
      const label = views.find((v) => v.node.id === roots[0])?.label ?? "";

      let revision = revisionRef.current;
      let deleted = 0;
      try {
        for (let i = 0; i < ordered.length; i += 1) {
          await deletePageMapNode(pageId, ordered[i], { base_revision: revision });
          deleted += 1;
          if (i + 1 < ordered.length) revision = (await getPageMap(pageId)).revision;
        }
      } catch (e) {
        // Every tombstone already written is terminal. Stopping here is the
        // right call — the rest of the subtree may no longer be what the user
        // selected — but the boxes that did go are not coming back, so the
        // count travels with the error instead of being swallowed by a notice
        // that says only "conflict".
        throw new PartialSubtreeDelete(e, label, deleted, ordered.length);
      }
      return { label, count: ordered.length };
    },
    [views, pageId],
  );

  const removeMutation = useMutation({
    mutationFn: removeSubtree,
    // Say what was destroyed, and that it is destroyed. Deleting a box takes
    // its whole subtree and the daemon's tombstones are terminal — the section
    // cannot be re-added under the same name — so a bare Backspace on a
    // selected box is the most expensive keystroke on this surface. It used to
    // be the quietest too: `onMutated` clears the notice area, so success said
    // nothing at all.
    //
    // The notice goes after `onMutated` for exactly that reason; before it, the
    // clear would swallow the message.
    onSuccess: ({ label, count }) => {
      onMutated();
      setNotice(
        count > 1
          ? t("pageCanvas.deletedWith", { label, count: count - 1 })
          : t("pageCanvas.deletedOne", { label }),
      );
    },
    onError: (error: unknown) => {
      if (error instanceof PartialSubtreeDelete && error.deleted > 0) {
        // The map on screen is stale either way, so refresh it exactly as the
        // shared handler would — then say what the refresh is about to reveal.
        void invalidate();
        void refetch();
        setNotice(
          t("pageCanvas.deletedPartial", {
            label: error.label,
            count: error.deleted,
            remaining: error.total - error.deleted,
          }),
        );
        return;
      }
      handleMutationError(error instanceof PartialSubtreeDelete ? error.reason : error);
    },
  });

  // A line drawn from the page is the page: renaming, adding or deleting one
  // writes the page itself, the way the editor does, and only onto the
  // version the map was read from. Each write can be taken back.
  const undoRef = useRef<{ before: string; after: string }[]>([]);
  const [undoOffer, setUndoOffer] = useState(false);
  useEffect(() => {
    undoRef.current = [];
    setUndoOffer(false);
  }, [pageId]);
  const writePage = useCallback(
    async (content: string, expectedVersion: number) => {
      const outcome = await updatePage({
        id: pageId,
        content,
        expectedVersion,
        callerId: "wenlan-app",
        operationId: globalThis.crypto.randomUUID(),
      });
      if (outcome.outcome === "conflict") throw new LineEditRefused("moved");
      if (outcome.outcome !== "saved") throw new LineEditRefused("failed");
    },
    [pageId],
  );
  const refreshPage = useCallback(() => {
    void queryClient.invalidateQueries({ queryKey: ["page-map", pageId] });
    void queryClient.invalidateQueries({ queryKey: ["page", pageId] });
    void queryClient.invalidateQueries({ queryKey: ["pages"] });
  }, [queryClient, pageId]);
  const lineEditMutation = useMutation({
    mutationFn: async (edit: OutlineEdit) => {
      const expected = map?.page_version;
      const page = await getPage(pageId);
      if (!page || expected == null || page.version !== expected) throw new LineEditRefused("moved");
      const raw = page.content.split(/\r?\n/)[edit.line] ?? "";
      if (edit.op === "rename" && INLINE_MARKS.test(raw)) throw new LineEditRefused("notEditable");
      const next = applyOutlineEdit(page.content, edit);
      if (next === null) throw new LineEditRefused("notEditable");
      if (next === page.content) return;
      await writePage(next, page.version);
      undoRef.current.push({ before: page.content, after: next });
    },
    onSuccess: () => {
      setUndoOffer(undoRef.current.length > 0);
      setNotice(undoRef.current.length > 0 ? t("pageCanvas.lineEdited") : null);
    },
    onError: (error) => {
      const reason = error instanceof LineEditRefused ? error.reason : "failed";
      setNotice(
        t(
          reason === "moved"
            ? "pageCanvas.lineMoved"
            : reason === "notEditable"
              ? "pageCanvas.lineNotEditable"
              : "pageCanvas.lineEditFailed",
        ),
      );
    },
    onSettled: () => {
      setPending(null);
      refreshPage();
    },
  });
  const { mutate: mutateLine } = lineEditMutation;
  const undoLineEdit = useCallback(async () => {
    const last = undoRef.current.pop();
    setUndoOffer(undoRef.current.length > 0);
    if (!last) return;
    try {
      const page = await getPage(pageId);
      // Only put back what the map changed: anything written since stays.
      if (!page || page.content !== last.after) throw new LineEditRefused("moved");
      await writePage(last.before, page.version);
      setNotice(t("pageCanvas.lineUndone"));
    } catch (error) {
      setNotice(
        t(error instanceof LineEditRefused && error.reason === "moved" ? "pageCanvas.lineMoved" : "pageCanvas.lineEditFailed"),
      );
    } finally {
      refreshPage();
    }
  }, [pageId, writePage, refreshPage, t]);
  // Whether a line of the page can be changed from here right now.
  const lineEditable = useCallback(
    (id: string) => {
      if (readOnly || map?.page_version == null) return null;
      const line = outlineLine(map, id);
      return line && line.line != null ? line : null;
    },
    [readOnly, map],
  );
  // Unsaved typing in the editor would be overwritten by a write from here.
  const linesPaused = useCallback(() => {
    if (!pageEditDirty) return false;
    setNotice(t("pageCanvas.lineEditPaused"));
    return true;
  }, [pageEditDirty, t]);
  const editLine = useCallback(
    (edit: OutlineEdit) => {
      if (linesPaused()) return;
      setMenu(null);
      mutateLine(edit);
    },
    [linesPaused, mutateLine],
  );
  // A moved line keeps the selection: after the page is reread, the line with
  // the same words is picked again, so Option-arrow can be pressed repeatedly.
  const [moveFollow, setMoveFollow] = useState<{ label: string; version: number } | null>(null);
  useEffect(() => {
    if (!moveFollow || map?.page_version == null || map.page_version === moveFollow.version) return;
    const found = (map.outline ?? []).find((line) => line.label === moveFollow.label);
    setMoveFollow(null);
    if (found) setRevealId(`${PAGE_LINE_PREFIX}${found.id}`);
  }, [moveFollow, map]);
  const moveLine = useCallback(
    (sourceId: string, targetId: string, place: MovePlace) => {
      const source = lineEditable(sourceId);
      const target = lineEditable(targetId);
      if (!source || !target || source.line == null || target.line == null) return;
      if (map?.page_version != null) setMoveFollow({ label: source.label, version: map.page_version });
      editLine({ op: "move", line: source.line, target: target.line, place });
    },
    [lineEditable, editLine, map],
  );
  // Option-arrow: past the neighbour of the same kind under the same heading.
  const moveLineStep = useCallback(
    (id: string, direction: 1 | -1): boolean => {
      const line = lineEditable(id);
      if (!line) return false;
      const row = (map?.outline ?? []).filter(
        (l) => l.parent_id === line.parent_id && (l.kind === "heading") === (line.kind === "heading"),
      );
      const next = row[row.findIndex((l) => l.id === line.id) + direction];
      if (next) moveLine(id, `${PAGE_LINE_PREFIX}${next.id}`, direction === 1 ? "after" : "before");
      return true;
    },
    [lineEditable, map, moveLine],
  );
  // Where a line dragged to `point` would land: above or below an item, or
  // above, into or below a heading. Never inside what it carries.
  const dropFor = useCallback(
    (sourceId: string, point: { x: number; y: number }): { id: string; place: MovePlace } | null => {
      const source = lineEditable(sourceId);
      if (!source || !Number.isFinite(point.x) || !Number.isFinite(point.y)) return null;
      for (const v of views) {
        if (v.node.id === sourceId) continue;
        if (Math.abs(point.x - v.x) > v.width / 2 || Math.abs(point.y - v.y) > v.height / 2 + 4) continue;
        const target = lineEditable(v.node.id);
        if (!target || lineAncestors(map, target.id).includes(source.id)) return null;
        const rel = (point.y - (v.y - v.height / 2)) / v.height;
        if (target.kind === "heading") {
          if (source.kind !== "heading") return { id: v.node.id, place: "under" };
          return { id: v.node.id, place: rel < 1 / 3 ? "before" : rel > 2 / 3 ? "after" : "under" };
        }
        if (source.kind === "heading") return null;
        return { id: v.node.id, place: rel < 0.5 ? "before" : "after" };
      }
      return null;
    },
    [lineEditable, views, map],
  );
  const [drop, setDrop] = useState<{ id: string; place: MovePlace } | null>(null);
  // The words being renamed, read from the page itself: the box shows a
  // shortened label, and renaming must start from what the page says.
  const [lineRename, setLineRename] = useState<{ id: string; line: number; text: string } | null>(null);
  const lineRenameRef = useRef(lineRename);
  lineRenameRef.current = lineRename;
  const startLineRename = useCallback(
    async (id: string) => {
      const line = lineEditable(id);
      if (!line || line.line == null || linesPaused()) return;
      const at = line.line;
      try {
        const page = await getPage(pageId);
        if (!page || page.version !== map?.page_version) {
          setNotice(t("pageCanvas.lineMoved"));
          refreshPage();
          return;
        }
        const raw = page.content.split(/\r?\n/)[at];
        const text = raw !== undefined && !INLINE_MARKS.test(raw) ? editableText(raw) : null;
        if (text === null) {
          setNotice(t("pageCanvas.lineNotEditable"));
          return;
        }
        setMenu(null);
        setNotice(null);
        setLineRename({ id, line: at, text });
        setEditingId(id);
      } catch {
        setNotice(t("pageCanvas.lineEditFailed"));
      }
    },
    [lineEditable, linesPaused, pageId, map?.page_version, refreshPage, t],
  );

  // Every canvas add gesture creates an independent idea. It never writes the
  // page body, and uses the fresh map revision plus the draft's stable ref_id so
  // a retry after an uncertain response cannot create a second idea.
  const addMutation = useMutation({
    mutationFn: async ({
      targetPageId,
      targetPageTitle,
      parentId,
      label,
      x,
      y,
      refId,
    }: {
      targetPageId: string;
      targetPageTitle: string;
      parentId: string;
      label: string;
      x: number;
      y: number;
      refId: string;
    }) => {
      const freshMap = await getPageMap(targetPageId);
      if (
        activePageRef.current.pageId !== targetPageId ||
        activePageRef.current.pageTitle !== targetPageTitle
      ) return null;
      if (!supportsIndependentIdeas(freshMap)) {
        throw new Error("independent ideas are unavailable on this runtime");
      }
      const parent = freshMap.nodes.find(
        (node) => node.id === parentId && node.status === "active",
      );
      if (!parent) throw new Error("idea parent is no longer active");
      let created = await createPageMapNode(targetPageId, {
        base_revision: freshMap.revision,
        parent_id: parentId,
        ref_kind: "idea",
        ref_id: refId,
        label,
      });
      if (created.node.label !== label) {
        created = await patchPageMapNode(targetPageId, created.node.id, {
          base_revision: created.revision,
          label,
        });
      }
      // Where the box was let go of is the whole statement of where it belongs,
      // and create cannot carry a position — so write it here, before anything
      // refetches. A node that arrives unplaced is given a computed ring slot,
      // which is the box jumping out from under the pointer a beat after it
      // was made. Keep the center and take the real width for the label, so it
      // settles around the drop point instead of away from it.
      const size = nodeBoxSize(label);
      try {
        await putPageMapLayout(targetPageId, {
          base_revision: created.revision,
          positions: [
            {
              node_id: created.node.id,
              x: x + DRAFT_SIZE.width / 2,
              y: y + DRAFT_SIZE.height / 2,
              width: size.width,
              height: size.height,
              collapsed: false,
            },
          ],
        });
      } catch {
        // The box exists; only its position was lost. Landing in the computed
        // slot is worse than staying put, but it is not a failed create and
        // must not be reported as one.
      }
      return created;
    },
    onSuccess: async (created, variables) => {
      if (
        !created ||
        activePageRef.current.pageId !== variables.targetPageId ||
        activePageRef.current.pageTitle !== variables.targetPageTitle
      ) return;
      setNotice(null);
      // Held until the refetch lands: clearing first would blink the box out and
      // back in as the server copy takes its place.
      await queryClient.invalidateQueries({ queryKey: ["page-map", variables.targetPageId] });
      setPending(null);
    },
    onError: (error: unknown, variables) => {
      if (
        activePageRef.current.pageId !== variables.targetPageId ||
        activePageRef.current.pageTitle !== variables.targetPageTitle
      ) return;
      setPending(null);
      setDraft({
        parentId: variables.parentId,
        label: variables.label,
        x: variables.x,
        y: variables.y,
        refId: variables.refId,
      });
      if (error instanceof Error && error.message.includes("unavailable")) {
        setNotice(t("pageCanvas.independentIdeasUnavailable"));
      } else if (asPageMapApiError(error)?.status === 409) {
        void queryClient.invalidateQueries({ queryKey: ["page-map", variables.targetPageId] });
        void refetch();
        setNotice(t("pageCanvas.conflict"));
      } else {
        handleMutationError(error);
      }
    },
  });

  const addIdeaMutation = useMutation({
    mutationFn: async ({ targetPageId, targetPageTitle, label, refId }: {
      targetPageId: string;
      targetPageTitle: string;
      label: string;
      refId: string;
    }) => {
      const freshMap = await getPageMap(targetPageId);
      if (
        activePageRef.current.pageId !== targetPageId ||
        activePageRef.current.pageTitle !== targetPageTitle
      ) return null;
      if (!supportsIndependentIdeas(freshMap)) {
        throw new Error("independent ideas are unavailable on this runtime");
      }
      const root = freshMap.nodes.find((node) => node.parent_id === null && node.status !== "dismissed");
      if (!root && (freshMap.revision !== 0 || freshMap.nodes.length !== 0)) {
        throw new Error("page map has no usable root");
      }
      const created = await createPageMapNode(targetPageId, {
        base_revision: freshMap.revision,
        ...(root ? { parent_id: root.id } : {}),
        ref_kind: "idea",
        ref_id: refId,
        label,
      });
      if (created.node.label !== label) {
        return patchPageMapNode(targetPageId, created.node.id, {
          base_revision: created.revision,
          label,
        });
      }
      return created;
    },
    onSuccess: async (created, variables) => {
      if (
        !created ||
        activePageRef.current.pageId !== variables.targetPageId ||
        activePageRef.current.pageTitle !== variables.targetPageTitle
      ) return;
      setIdeaError(null);
      setIdeaDraft("");
      refocusSurfaceRef.current = true;
      setIdeaEntryOpen(false);
      setIdeaEntryAnchor(null);
      ideaDraftRefId.current = null;
      ideaSubmissionAttemptedRef.current = false;
      await invalidate();
    },
    onError: (error, variables) => {
      if (
        activePageRef.current.pageId !== variables.targetPageId ||
        activePageRef.current.pageTitle !== variables.targetPageTitle
      ) return;
      setIdeaError(
        error instanceof Error && error.message.includes("unavailable")
          ? t("pageCanvas.independentIdeasUnavailable")
          : asPageMapApiError(error)?.status === 409
            ? t("pageCanvas.conflict")
            : t("pageCanvas.mutationError"),
      );
    },
  });

  const submitIdea = (event: React.FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const label = ideaDraft.trim();
    if (!label || readOnly || !independentIdeasSupported || addIdeaMutation.isPending) return;
    const refId = ideaDraftRefId.current ?? globalThis.crypto.randomUUID();
    ideaDraftRefId.current = refId;
    ideaSubmissionAttemptedRef.current = true;
    setIdeaError(null);
    addIdeaMutation.mutate({ targetPageId: pageId, targetPageTitle: pageTitle, label, refId });
  };

  const dismissIdeaEntry = () => {
    if (addIdeaMutation.isPending) return;
    refocusSurfaceRef.current = true;
    setIdeaEntryOpen(false);
    setIdeaEntryAnchor(null);
    if (!ideaSubmissionAttemptedRef.current) {
      setIdeaDraft("");
      ideaDraftRefId.current = null;
      setIdeaError(null);
    }
  };

  const ideaEntry = ideaEntryOpen && ideaEntryAnchor ? (
    <form
      ref={ideaEntryRef}
      id={`${ideaNameInputId}-entry`}
      className="page-canvas-idea-entry page-canvas-empty-entry"
      style={{ left: ideaEntryAnchor.x, top: ideaEntryAnchor.y }}
      onSubmit={submitIdea}
      aria-label={t("pageCanvas.addNode")}
      onContextMenu={(event) => {
        event.preventDefault();
        event.stopPropagation();
      }}
      onKeyDown={(event) => {
        if (event.key === "Escape") {
          event.preventDefault();
          event.stopPropagation();
          dismissIdeaEntry();
        }
      }}
    >
      <label className="sr-only" htmlFor={ideaNameInputId}>{t("pageCanvas.nodeNameLabel")}</label>
      <input
        id={ideaNameInputId}
        value={ideaDraft}
        onChange={(event) => {
          const value = event.currentTarget.value;
          if (value.trim().length > 0) ideaDraftRefId.current ??= globalThis.crypto.randomUUID();
          setIdeaDraft(value);
          setIdeaError(null);
        }}
        disabled={!independentIdeasSupported || addIdeaMutation.isPending}
        placeholder={t("pageCanvas.nodeNameLabel")}
        autoComplete="off"
        autoFocus
      />
      <button type="submit" disabled={
        !independentIdeasSupported || addIdeaMutation.isPending || ideaDraft.trim().length === 0
      }>
        {t("pageCanvas.addNode")}
      </button>
      <button type="button" onClick={dismissIdeaEntry} disabled={addIdeaMutation.isPending}>
        {t("pageCanvas.cancelNode")}
      </button>
      {ideaError && <p className="page-canvas-idea-hint" role="alert">{ideaError}</p>}
    </form>
  ) : null;

  useLayoutEffect(() => {
    const form = ideaEntryRef.current;
    const surface = surfaceRef.current;
    if (!ideaEntryOpen || !form || !surface) return;
    const clampEntry = () => {
      const maxX = Math.max(8, surface.clientWidth - form.offsetWidth - 8);
      const maxY = Math.max(8, surface.clientHeight - form.offsetHeight - 8);
      setIdeaEntryAnchor((current) => {
        if (!current) return current;
        const x = Math.min(Math.max(8, current.x), maxX);
        const y = Math.min(Math.max(8, current.y), maxY);
        return x === current.x && y === current.y ? current : { x, y };
      });
    };
    clampEntry();
    const observer = new ResizeObserver(clampEntry);
    observer.observe(form);
    observer.observe(surface);
    return () => observer.disconnect();
  }, [ideaEntryOpen, ideaError]);

  const layoutTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const flushLayout = useCallback(async () => {
    // Snapshot the stamps this PUT is answering for, so a drop that lands
    // while it is in flight stays dirty.
    const flushing = new Map(dirtyRef.current);
    const meta = metaRef.current;
    const positions: PageMapLayoutPosition[] = [];
    for (const n of nodesRef.current) {
      const m = meta.get(n.id);
      if (!m) continue; // the draft box, or a node the last refetch dropped
      // React Flow positions are top-left; the daemon stores box centers, so a
      // round-trip through layoutMap lands where the user let go.
      positions.push({
        node_id: n.id,
        x: n.position.x + m.width / 2,
        y: n.position.y + m.height / 2,
        width: m.width,
        height: m.height,
        collapsed: m.collapsed,
      });
    }
    if (positions.length === 0) return;
    const viewport = getViewport();
    const put = (base_revision: number) =>
      putPageMapLayout(pageId, { base_revision, viewport, positions });

    try {
      await put(revisionRef.current);
    } catch (e) {
      if (asPageMapApiError(e)?.status !== 409) {
        setNotice(t("pageCanvas.mutationError"));
        return;
      }
      // Where the user dropped a node IS the intent, so it wins the race:
      // refetch the revision and replay the same coordinates exactly once.
      const fresh = await refetch();
      const rev = fresh.data?.revision;
      if (rev === undefined) {
        setNotice(t("pageCanvas.conflict"));
        return;
      }
      try {
        await put(rev);
      } catch {
        setNotice(t("pageCanvas.conflict"));
        return;
      }
    }
    for (const [id, stamp] of flushing) {
      if (dirtyRef.current.get(id) === stamp) dirtyRef.current.delete(id);
    }
    setNotice(null);
    void invalidate();
  }, [getViewport, pageId, refetch, invalidate, t]);

  // Unmount inside the debounce window used to drop the drag on the floor:
  // move a box, then hit Edit page or the canvas toggle within 600ms, and the
  // position was gone. The teardown flushes instead of just cancelling, so the
  // last thing the user did is the thing that gets saved. Read through a ref
  // because this effect must run only on unmount, and flushLayout is a fresh
  // closure on every render.
  const flushLayoutRef = useRef(flushLayout);
  flushLayoutRef.current = flushLayout;
  useEffect(
    () => () => {
      if (!layoutTimer.current) return;
      clearTimeout(layoutTimer.current);
      layoutTimer.current = null;
      // Nothing is left to render the outcome, so this is deliberately
      // fire-and-forget: the PUT still goes out, and a failure surfaces on the
      // next open as the stale position it is.
      void flushLayoutRef.current();
    },
    [],
  );

  // One PUT per burst: every move restarts the timer, so dragging five nodes in
  // a row — or holding an arrow key down — sends one request carrying them all.
  const markMoved = useCallback(
    (ids: string[]) => {
      for (const id of ids) {
        dragSeq.current += 1;
        dirtyRef.current.set(id, dragSeq.current);
      }
      if (layoutTimer.current) clearTimeout(layoutTimer.current);
      layoutTimer.current = setTimeout(() => {
        layoutTimer.current = null;
        void flushLayout();
      }, LAYOUT_DEBOUNCE_MS);
    },
    [flushLayout],
  );

  const dragPoint = useCallback(
    (event: MouseEvent | TouchEvent | ReactMouseEvent) => {
      const touch = "changedTouches" in event ? event.changedTouches[0] : null;
      const at = touch ?? (event as MouseEvent);
      return screenToFlowPosition({ x: at.clientX, y: at.clientY });
    },
    [screenToFlowPosition],
  );
  const handleNodeDrag = useCallback(
    (event: MouseEvent | TouchEvent | ReactMouseEvent, node: Node) => {
      if (!isPageLine(node.id)) return;
      const next = dropFor(node.id, dragPoint(event));
      setDrop((current) =>
        current?.id === next?.id && current?.place === next?.place ? current : next,
      );
    },
    [dropFor, dragPoint],
  );
  const handleNodeDragStop = useCallback(
    (event: MouseEvent | TouchEvent | ReactMouseEvent, node: Node) => {
      if (!isPageLine(node.id)) {
        markMoved([node.id]);
        return;
      }
      // A line of the page goes back to its place in the page's order; where
      // it was dropped decides only where in the page it moves to.
      setDrop(null);
      const placed = views.find((v) => v.node.id === node.id);
      if (placed) {
        const home = { x: placed.x - placed.width / 2, y: placed.y - placed.height / 2 };
        setNodes((ns) => ns.map((n) => (n.id === node.id ? { ...n, position: home } : n)));
      }
      const target = dropFor(node.id, dragPoint(event));
      if (target) moveLine(node.id, target.id, target.place);
    },
    [markMoved, views, setNodes, dropFor, dragPoint, moveLine],
  );

  // A line of the page opens the part of the page it came from; "show the
  // rest" unfolds its heading; the page box itself has nowhere else to go.
  const openPageLine = useCallback(
    (id: string) => {
      const heading = moreFor(id);
      if (heading !== null) {
        toggleHeading(heading);
        return;
      }
      const line = outlineLine(map, id);
      if (!line) return;
      if (line.kind === "heading") {
        setFocusLine((current) => (current === line.id ? null : line.id));
        // WebKit does not focus a clicked button, so the Escape that should
        // let go of the branch would otherwise land outside the map and leave
        // the page instead.
        surfaceRef.current?.focus({ preventScroll: true });
      }
      const parent = line.kind === "heading"
        ? line
        : map?.outline?.find((l) => l.id === line.parent_id) ?? null;
      onSectionClick?.({ text: line.text, heading: parent?.text ?? null });
    },
    [map, onSectionClick, toggleHeading],
  );

  const openNode = useCallback(
    (node: PageMapNode) => {
      if (isPageLine(node.id)) return openPageLine(node.id);
      if (node.ref_kind === "memory") onMemoryClick(node.ref_id);
      else if (node.ref_kind === "page") onPageClick?.(node.ref_id);
      else if (node.ref_kind === "entity") onEntityClick?.(node.ref_id);
      // "section" and "idea" are pure grouping nodes with no backing page.
    },
    [onMemoryClick, onPageClick, onEntityClick, openPageLine],
  );

  const { mutate: mutateStatus } = statusMutation;
  const { mutate: mutateRename, mutateAsync: mutateRenameAsync } = renameMutation;
  const { mutate: mutateRemove } = removeMutation;
  const { mutate: mutateAdd } = addMutation;

  // Server-derived nodes. Rebuilt only when the map itself changes, never on a
  // drag frame — every callback below closes over stable identities so the
  // `data` object can survive a whole drag untouched.
  // Each top-level heading gets its own ink, shared by everything under it.
  const inks = useMemo(() => branchSlots(map), [map]);
  const inkOf = useCallback(
    (id: string) => {
      const lineId = lineIdOf(id);
      const slot = lineId ? inks.get(lineId) : undefined;
      return slot ? palette[slot] : undefined;
    },
    [inks, palette],
  );
  const isDimmed = useCallback(
    (id: string, isRoot: boolean) => {
      if (!focusLine || isRoot) return false;
      const lineId = lineIdOf(id);
      return !(lineId && withinLine(map, focusLine, lineId));
    },
    [focusLine, map],
  );

  const serverNodes = useMemo<Node[]>(
    () =>
      views.map((v) => {
        const pageLine = isPageLine(v.node.id);
        const line = pageLine ? outlineLine(map, v.node.id) : null;
        const foldId = line?.kind === "heading" && foldable.has(line.id) ? line.id : null;
        const data: CanvasNodeData = {
          label: v.label,
          refKind: v.node.ref_kind,
          status: v.node.status,
          dangling: v.node.ref_state === "dangling",
          isRoot: v.node.parent_id === null,
          depth: v.depth,
          // Layout puts the page at the origin, so the box's own center already
          // points away from it.
          // The tree grows rightward, so the grow dot sits on the right.
          outward: 0,
          readOnly: readOnly || pageLine,
          hint: line && line.text !== v.label ? line.text : undefined,
          link: moreFor(v.node.id) !== null,
          unfolded: foldId ? branchOpen(foldId) : undefined,
          onToggleFold: foldId ? () => toggleBranch(foldId) : undefined,
          branchColor: inkOf(v.node.id),
          underline: pageLine && v.depth >= 2,
          dimmed: isDimmed(v.node.id, v.node.parent_id === null),
          palette,
          width: v.width,
          height: v.height,
          editing: false,
          nameInputLabel: v.node.ref_kind === "idea" ? t("pageCanvas.nodeNameLabel") : undefined,
          onOpen: () => openNode(v.node),
          onAccept: () => mutateStatus({ nodeId: v.node.id, status: "active" }),
          onDismiss: () => mutateStatus({ nodeId: v.node.id, status: "dismissed" }),
          onCommit: (label: string) => {
            const trimmed = label.trim();
            if (pageLine) {
              const renaming = lineRenameRef.current;
              setEditingId(null);
              setLineRename(null);
              if (renaming && renaming.id === v.node.id && trimmed && trimmed !== renaming.text.trim()) {
                editLine({ op: "rename", line: renaming.line, text: trimmed });
              }
              return;
            }
            if (!trimmed || trimmed === v.label) {
              setEditingId(null);
              return;
            }
            return mutateRenameAsync({ nodeId: v.node.id, label: trimmed }).then(() => undefined);
          },
          onCancel: () => {
            setEditingId(null);
            setLineRename(null);
          },
        };
        return {
          id: v.node.id,
          type: "pageMapNode",
          // The page's lines are laid out by the page's own order.
          // The narrow list is not a canvas to arrange; the wide tree is.
          // A line of the page can be dragged to another place in the page.
          draggable: !readOnly && (pageLine ? !!lineEditable(v.node.id) : layoutMode !== "indent"),
          connectable: !readOnly && !moreFor(v.node.id),
          position: { x: v.x - v.width / 2, y: v.y - v.height / 2 },
          data,
        };
      }),
    [views, readOnly, palette, openNode, mutateStatus, mutateRename, mutateRenameAsync, t, map, foldable, branchOpen, toggleBranch, layoutMode, inkOf, isDimmed, editLine, lineEditable],
  );

  // Fold the server's copy into local state, keeping what the user has done
  // since: an unflushed drag outranks the stored position, and selection must
  // survive a refetch or the map deselects itself every time it saves.
  //
  // The measured size survives too. React Flow hides a box that has none until
  // it has measured it again, and a hidden box takes no pointer input, so
  // without it every save made every box blink out for a frame and a click or
  // double-click arriving in that frame went to the canvas behind the box. A
  // box whose size really changed is measured again by React Flow's own
  // observer.
  useEffect(() => {
    setNodes((prev) => {
      const prevById = new Map(prev.map((n) => [n.id, n]));
      return serverNodes.map((n) => {
        const old = prevById.get(n.id);
        if (!old) return n;
        return {
          ...n,
          position: dirtyRef.current.has(n.id) ? old.position : n.position,
          selected: old.selected,
          measured: old.measured,
        };
      });
    });
  }, [serverNodes, setNodes]);

  // Declared after the sync above so a line drawn just now is in `nodes` by the
  // time it is selected.
  useEffect(() => {
    if (!revealId) return;
    const at = views.find((v) => v.node.id === revealId);
    if (!at) return;
    setRevealId(null);
    setNodes((ns) => ns.map((n) => (n.selected !== (n.id === revealId) ? { ...n, selected: n.id === revealId } : n)));
    void setCenter(at.x, at.y, { zoom: Math.max(getZoom(), 0.9), duration: 200 });
  }, [revealId, views, setNodes, setCenter, getZoom]);

  const commitDraft = useCallback(
    (label: string) => {
      const pending = draft;
      if (!pending) return;
      const trimmed = label.trim();
      // An empty commit is never someone deciding against the box: the field
      // blurs on its own when React Flow takes focus back for the pane after a
      // connector drag, and clearing the draft here deleted the box a beat
      // after it appeared. Leave it standing — Escape is how you say no.
      if (!trimmed) return;
      setDraft(null);
      setPending({ ...pending, label: trimmed });
      if (pending.line) {
        // A new line of the page: the box stays drawn until the page is reread.
        mutateLine({ op: pending.line.op, line: pending.line.line, text: trimmed });
        return;
      }
      mutateAdd({
        targetPageId: pageId,
        targetPageTitle: pageTitle,
        parentId: pending.parentId,
        label: trimmed,
        x: pending.x,
        y: pending.y,
        refId: pending.refId,
      });
    },
    [draft, mutateAdd, mutateLine, pageId, pageTitle],
  );

  // One box, two moments: being named, then waiting for the daemon. It keeps the
  // same id and position through both so it never blinks out between them.
  const draftNode = useMemo<Node | null>(() => {
    const spot = draft ?? pending;
    if (!spot) return null;
    const naming = draft !== null;
    const data: CanvasNodeData = {
      label: naming ? (draft?.label ?? "") : (pending?.label ?? ""),
      refKind: "idea",
      status: "active",
      dangling: false,
      isRoot: false,
      // Dress it as what it is about to become, so it does not restyle itself
      // the instant the server's copy arrives.
      depth: (views.find((v) => v.node.id === spot.parentId)?.depth ?? 0) + 1,
      outward: Math.atan2(
        spot.y + DRAFT_SIZE.height / 2,
        spot.x + DRAFT_SIZE.width / 2,
      ),
      readOnly: false,
      palette,
      width: DRAFT_SIZE.width,
      height: DRAFT_SIZE.height,
      editing: naming,
      placeholder: t(spot.line ? "pageCanvas.newLinePlaceholder" : "pageCanvas.newSectionPlaceholder"),
      nameInputLabel: t(spot.line ? "pageCanvas.newLinePlaceholder" : "pageCanvas.newSectionPlaceholder"),
      onNameChange: (label: string) => {
        setDraft((current) => current ? { ...current, label } : current);
      },
      onOpen: () => {},
      onAccept: () => {},
      onDismiss: () => {},
      onCommit: commitDraft,
      onCancel: () => setDraft(null),
    };
    return {
      id: DRAFT_ID,
      type: "pageMapNode",
      draggable: false,
      selectable: false,
      position: { x: spot.x, y: spot.y },
      // The draft lives only in displayNodes, never in `nodes`, so the measure
      // React Flow reports for it is dropped by applyNodeChanges and it would
      // stay visibility:hidden forever — with an unfocusable name field inside
      // it. We already know the size, so say it and skip measuring.
      width: DRAFT_SIZE.width,
      height: DRAFT_SIZE.height,
      data,
    };
  }, [draft, pending, views, palette, commitDraft, t]);

  // The stored page box. Null while nothing is stored, even when the page's
  // own lines are drawn around a stand-in page box.
  const rootId = storedRootId;

  const localPoint = useCallback((clientX: number, clientY: number) => {
    const box = surfaceRef.current?.getBoundingClientRect();
    return { x: clientX - (box?.left ?? 0), y: clientY - (box?.top ?? 0) };
  }, []);

  const openEmptyIdeaAt = useCallback((x: number, y: number) => {
    const surface = surfaceRef.current;
    const width = surface?.clientWidth ?? 320;
    const height = surface?.clientHeight ?? 240;
    const formWidth = Math.min(280, Math.max(120, width - 16));
    const left = Math.min(Math.max(8, x), Math.max(8, width - formWidth - 8));
    const top = Math.min(Math.max(8, y + 8), Math.max(8, height - 140));
    setIdeaError(null);
    setMenu(null);
    setIdeaEntryAnchor({ x: left, y: top });
    refocusSurfaceRef.current = false;
    setIdeaEntryOpen(true);
    return false;
  }, []);

  const startDraftAt = useCallback((parentId: string, x: number, y: number) => {
    if (readOnly) return;
    if (!independentIdeasSupported) {
      setNotice(t("pageCanvas.independentIdeasUnavailable"));
      return;
    }
    if (isPageLine(parentId)) {
      // The page's lines live in the page, not in the map, so a box of the
      // person's own hangs off the page box instead.
      if (rootId) {
        parentId = rootId;
      } else {
        const surface = surfaceRef.current;
        openEmptyIdeaAt((surface?.clientWidth ?? 320) / 2, (surface?.clientHeight ?? 240) / 2);
        return;
      }
    }
    setEditingId(null);
    setMenu(null);
    // Why the last thing failed has nothing to do with the box being drawn now.
    // Left up, "The center box is the page itself" sat over a brand-new box and
    // read as a complaint about that one.
    setNotice(null);
    setDraft({ parentId, x, y, label: "", refId: globalThis.crypto.randomUUID() });
  }, [readOnly, independentIdeasSupported, t, rootId, openEmptyIdeaAt]);

  const startDraft = useCallback(
    (mode: "child" | "sibling") => {
      const selected = nodesRef.current.find((n) => n.selected);
      const line = selected ? lineEditable(selected.id) : null;
      if (selected && line && line.line != null) {
        // On a line of the page, Tab and Enter write a new line of the page:
        // Tab goes inside a heading, Enter (or Tab on an item) adds the next one.
        if (linesPaused()) return;
        const op = mode === "child" && line.kind === "heading" ? "addUnder" : "addAfter";
        const parentId = op === "addUnder"
          ? selected.id
          : line.parent_id
            ? `${PAGE_LINE_PREFIX}${line.parent_id}`
            : (rootId ?? PAGE_LINE_ROOT);
        setEditingId(null);
        setMenu(null);
        setNotice(null);
        setDraft({
          parentId,
          x: selected.position.x + DRAFT_OFFSET.x,
          y: selected.position.y + DRAFT_OFFSET.y,
          label: "",
          refId: globalThis.crypto.randomUUID(),
          line: { op, line: line.line },
        });
        return;
      }
      if (!rootId && !selected) return;
      const anchorId = selected?.id ?? (rootId as string);
      const parentId =
        !selected || mode === "child"
          ? anchorId
          : (views.find((v) => v.node.id === anchorId)?.node.parent_id ?? rootId ?? anchorId);
      const base =
        nodesRef.current.find((n) => n.id === anchorId)?.position ?? { x: 0, y: 0 };
      startDraftAt(parentId, base.x + DRAFT_OFFSET.x, base.y + DRAFT_OFFSET.y);
    },
    [views, rootId, startDraftAt, lineEditable, linesPaused],
  );

  // A box drawn on empty canvas still has to join the tree — every node here
  // carries a parent — so it hangs off whatever is selected, or off the page
  // itself. It lands centered on the pointer, where the user aimed.
  const addBoxAt = useCallback(
    (clientX: number, clientY: number, parentId?: string) => {
      if (readOnly) return;
      if (!rootId) {
        const point = localPoint(clientX, clientY);
        openEmptyIdeaAt(point.x, point.y);
        return;
      }
      if (!independentIdeasSupported) {
        setNotice(t("pageCanvas.independentIdeasUnavailable"));
        return;
      }
      const surface = surfaceRef.current;
      const bounds = surface?.getBoundingClientRect();
      const zoom = getViewport().zoom || 1;
      const halfWidth = Math.min(DRAFT_SIZE.width * zoom / 2, Math.max(0, (surface?.clientWidth ?? 0) / 2 - 8));
      const halfHeight = Math.min(DRAFT_SIZE.height * zoom / 2, Math.max(0, (surface?.clientHeight ?? 0) / 2 - 8));
      const x = bounds ? Math.min(Math.max(clientX, bounds.left + halfWidth + 8), bounds.right - halfWidth - 8) : clientX;
      const y = bounds ? Math.min(Math.max(clientY, bounds.top + halfHeight + 8), bounds.bottom - halfHeight - 8) : clientY;
      const point = screenToFlowPosition({ x, y });
      const selected = nodesRef.current.find((n) => n.selected && n.id !== DRAFT_ID);
      startDraftAt(
        parentId ?? selected?.id ?? rootId,
        point.x - DRAFT_SIZE.width / 2,
        point.y - DRAFT_SIZE.height / 2,
      );
    },
    [readOnly, independentIdeasSupported, rootId, screenToFlowPosition, getViewport, startDraftAt, t, localPoint, openEmptyIdeaAt],
  );

  // Drag a box's connector into empty space and let go: the new box is already
  // a child of the one you dragged from. Dropping onto another box does nothing
  // — re-parenting by drag is a separate feature.
  const handleConnectEnd = useCallback(
    (event: MouseEvent | TouchEvent, state: FinalConnectionState) => {
      if (state.isValid) return;
      const from = state.fromNode?.id;
      if (!from || from === DRAFT_ID) return;
      // React Flow hands over the raw DOM event, which is the only thing here
      // carrying where the pointer was released.
      const point = "changedTouches" in event ? event.changedTouches[0] : event;
      if (!point) return;
      addBoxAt(point.clientX, point.clientY, from);
    },
    [addBoxAt],
  );

  const selectAll = useCallback(() => {
    setNodes((ns) =>
      ns.map((n) => (n.id === DRAFT_ID ? n : { ...n, selected: true })),
    );
  }, [setNodes]);

  const clearSelection = useCallback(() => {
    setNodes((ns) => ns.map((n) => (n.selected ? { ...n, selected: false } : n)));
  }, [setNodes]);

  // Arrow keys walk a page's lines the way an outline reads: up and down
  // between neighbours, left to the heading above, right into what is under
  // it. A box of the person's own still moves with them instead.
  const stepSelection = useCallback(
    (key: string): boolean => {
      const picked = nodesRef.current.filter((n) => n.selected);
      if (picked.length !== 1) return false;
      const at = views.find((v) => v.node.id === picked[0].id);
      if (!at || !(isPageLine(at.node.id) || (at.node.parent_id === null && hasPageLines))) return false;
      const byY = (a: { y: number }, b: { y: number }) => a.y - b.y;
      let to: string | null | undefined = null;
      if (key === "ArrowLeft") to = at.node.parent_id;
      else if (key === "ArrowRight") {
        const kids = views.filter((v) => v.node.parent_id === at.node.id).sort(byY);
        const line = lineIdOf(at.node.id);
        if (!kids.length && line && foldable.has(line) && !branchOpen(line)) {
          toggleBranch(line);
          return true;
        }
        to = kids[0]?.node.id;
      } else {
        const row = views.filter((v) => v.node.parent_id === at.node.parent_id).sort(byY);
        const i = row.findIndex((v) => v.node.id === at.node.id);
        to = row[i + (key === "ArrowDown" ? 1 : -1)]?.node.id;
      }
      if (to && views.some((v) => v.node.id === to)) {
        setNodes((ns) => ns.map((n) => (n.selected !== (n.id === to) ? { ...n, selected: n.id === to } : n)));
      }
      return true;
    },
    [views, foldable, branchOpen, toggleBranch, setNodes, hasPageLines],
  );

  const nudge = useCallback(
    (dx: number, dy: number) => {
      const ids = nodesRef.current
        .filter((n) => n.selected && n.id !== DRAFT_ID && !isPageLine(n.id))
        .map((n) => n.id);
      if (ids.length === 0) return;
      const moving = new Set(ids);
      setNodes((ns) =>
        ns.map((n) =>
          moving.has(n.id)
            ? { ...n, position: { x: n.position.x + dx, y: n.position.y + dy } }
            : n,
        ),
      );
      markMoved(ids);
    },
    [setNodes, markMoved],
  );

  const deleteSelection = useCallback(() => {
    const picked = nodesRef.current.filter((n) => n.selected && n.id !== DRAFT_ID);
    const line = picked.length === 1 ? lineEditable(picked[0].id) : null;
    if (line && line.line != null) {
      // One line of the page: it leaves the page, with everything it holds.
      editLine({ op: "delete", line: line.line });
      return;
    }
    const selected = nodesRef.current
      .filter((n) => n.selected && n.id !== DRAFT_ID && !isPageLine(n.id))
      .map((n) => n.id);
    if (selected.length === 0) return;
    const roots = new Set(
      views.filter((v) => v.node.parent_id === null).map((v) => v.node.id),
    );
    const deletable = selected.filter((id) => !roots.has(id));
    // Selecting everything and hitting Delete should still clear the map around
    // the center box, so this only complains when the center box was the *whole*
    // selection.
    if (deletable.length === 0) {
      setNotice(t("pageCanvas.rootUndeletable"));
      return;
    }
    setMenu(null);
    mutateRemove(deletable);
  }, [views, mutateRemove, t, lineEditable, editLine]);

  const restoreMenuFocus = useCallback((restore = true) => {
    setMenu(null);
    if (!restore) return;
    requestAnimationFrame(() => {
      const target = menuReturnFocusRef.current;
      (target?.isConnected ? target : surfaceRef.current)?.focus({ preventScroll: true });
    });
  }, []);

  const handlePaneContextMenu = useCallback(
    (event: ReactMouseEvent | MouseEvent) => {
      event.preventDefault(); // the browser's own menu has nothing to offer here
      const point = localPoint(event.clientX, event.clientY);
      menuReturnFocusRef.current = surfaceRef.current;
      setMenu({
        kind: "pane",
        x: point.x,
        y: point.y,
        clientX: event.clientX,
        clientY: event.clientY,
      });
    },
    [localPoint],
  );

  const handleNodeContextMenu = useCallback(
    (event: ReactMouseEvent, node: Node) => {
      event.preventDefault();
      if (node.id === DRAFT_ID) return;
      const point = localPoint(event.clientX, event.clientY);
      const target = event.target instanceof HTMLElement ? event.target : null;
      menuReturnFocusRef.current = target?.closest<HTMLElement>(".react-flow__node")?.querySelector<HTMLElement>("button") ?? surfaceRef.current;
      setMenu({ kind: "node", x: point.x, y: point.y, nodeId: node.id });
      // Right-clicking inside an existing multi-selection keeps it — that is the
      // whole point of "delete these four". Right-clicking outside one moves the
      // selection to the box under the pointer, so the menu's verbs have an
      // obvious referent.
      setNodes((ns) =>
        ns.some((n) => n.id === node.id && n.selected)
          ? ns
          : ns.map((n) => ({ ...n, selected: n.id === node.id })),
      );
    },
    [localPoint, setNodes],
  );

  // Double-click on empty canvas draws a box; on a box it renames. React Flow
  // has no pane-doubleclick callback, so the surface listens and defers to the
  // node handler when the pointer was over one.
  const handleDoubleClick = useCallback(
    (event: ReactMouseEvent<HTMLDivElement>) => {
      // The actions, the badge and the shortcut disclosure all float inside the
      // surface, so a double-click on any of them would otherwise draw a box
      // behind the control the user was aiming at.
      const inert =
        ".react-flow__node, .page-canvas-actions, .page-canvas-help, .page-canvas-badge, .page-canvas-empty-entry";
      if ((event.target as HTMLElement).closest(inert)) return;
      addBoxAt(event.clientX, event.clientY);
    },
    [addBoxAt],
  );

  const handleNodeDoubleClick = useCallback(
    (_event: ReactMouseEvent, node: Node) => {
      if (node.id === DRAFT_ID) return;
      if (isPageLine(node.id)) {
        if (lineEditable(node.id)) void startLineRename(node.id);
        else openPageLine(node.id);
        return;
      }
      if (readOnly) return;
      setMenu(null);
      setEditingId(node.id);
    },
    [readOnly, openPageLine, lineEditable, startLineRename],
  );

  const menuItems = useMemo<MenuItem[]>(() => {
    if (!menu) return [];
    if (menu.kind === "pane") {
      const items: MenuItem[] = [];
      if (!readOnly) {
        items.push({
          key: "add",
          label: t("pageCanvas.menuAddHere"),
          disabled: !independentIdeasSupported,
          run: () => {
            if (isEmptyMap) return openEmptyIdeaAt(menu.x, menu.y);
            addBoxAt(menu.clientX, menu.clientY);
            return false;
          },
        });
        if (map && map.map_schema >= 1 && map.map_schema <= SUPPORTED_MAP_SCHEMA) {
          items.push({
            key: "generate",
            label: improveMutation.isPending ? t("pageCanvas.generating") : t("pageCanvas.addRelated"),
            disabled: improveMutation.isPending,
            run: () => { improveMutation.mutate(); },
          });
        }
      }
      items.push({ key: "all", label: t("pageCanvas.menuSelectAll"), run: selectAll });
      if (hasPageLines) {
        for (const level of [1, 2, 3]) {
          items.push({ key: `fold-${level}`, label: t("pageCanvas.menuFoldTo", { count: level }), run: () => foldToLevel(level) });
        }
        items.push({ key: "fold-all", label: t("pageCanvas.menuUnfoldAll"), run: () => foldToLevel(null) });
      }
      return items;
    }
    const view = views.find((v) => v.node.id === menu.nodeId);
    if (!view) return [];
    if (isPageLine(view.node.id)) {
      // A line of the page is changed in the page, so the map offers only the
      // way there.
      return outlineLine(map, view.node.id)
        ? [{ key: "open", label: t("pageCanvas.menuShowInPage"), run: () => { openPageLine(view.node.id); return false; } }]
        : [];
    }
    const items: MenuItem[] = [];
    // A section is a heading in the page body — there is nothing behind it to
    // open, so the verb is omitted rather than shown doing nothing.
    if (view.node.ref_kind !== "section" && view.node.ref_kind !== "idea") {
      items.push({
        key: "open",
        label: t("pageCanvas.menuOpen"),
        run: () => { openNode(view.node); return false; },
      });
    }
    if (readOnly) return items;
    const anchor = nodesRef.current.find((n) => n.id === menu.nodeId)?.position;
    items.push(
      {
        key: "child",
        label: t("pageCanvas.menuAddChild"),
        disabled: !independentIdeasSupported,
        run: () => {
          startDraftAt(
            menu.nodeId,
            (anchor?.x ?? 0) + DRAFT_OFFSET.x,
            (anchor?.y ?? 0) + DRAFT_OFFSET.y,
          );
          return false;
        },
      },
      {
        key: "rename",
        label: t("pageCanvas.menuRename"),
        run: () => {
          setMenu(null);
          setEditingId(menu.nodeId);
          return false;
        },
      },
    );
    if (view.node.parent_id !== null) {
      const kids = views.some((v) => v.node.parent_id === menu.nodeId);
      items.push({
        key: "delete",
        // Naming the cascade is the only warning the user gets — the boxes
        // underneath go too, and there is no undo yet.
        label: kids
          ? t("pageCanvas.menuDeleteSubtree")
          : t("pageCanvas.menuDelete"),
        run: deleteSelection,
        danger: true,
      });
    }
    return items;
  }, [
    menu,
    views,
    readOnly,
    independentIdeasSupported,
    isEmptyMap,
    map,
    improveMutation.isPending,
    improveMutation.mutate,
    t,
    addBoxAt,
    openEmptyIdeaAt,
    selectAll,
    openNode,
    openPageLine,
    startDraftAt,
    deleteSelection,
    hasPageLines,
    foldToLevel,
  ]);

  useLayoutEffect(() => {
    const surface = surfaceRef.current;
    const element = menuElementRef.current;
    if (!menu || !surface || !element) return;
    const maxX = Math.max(8, surface.clientWidth - element.offsetWidth - 8);
    const maxY = Math.max(8, surface.clientHeight - element.offsetHeight - 8);
    const x = Math.min(Math.max(8, menu.x), maxX);
    const y = Math.min(Math.max(8, menu.y), maxY);
    if (x !== menu.x || y !== menu.y) {
      setMenu((current) => current ? { ...current, x, y } : current);
    }
  }, [menu, menuItems]);

  const openKeyboardContextMenu = useCallback((target: HTMLElement) => {
    const nodeElement = target.closest<HTMLElement>(".react-flow__node");
    const rect = nodeElement?.getBoundingClientRect() ?? surfaceRef.current?.getBoundingClientRect();
    const clientX = rect ? rect.left + rect.width / 2 : 0;
    const clientY = rect ? rect.top + rect.height / 2 : 0;
    const point = localPoint(clientX, clientY);
    if (nodeElement) {
      const nodeId = nodeElement.getAttribute("data-id") ?? nodeElement.getAttribute("data-node-id");
      if (nodeId && nodesRef.current.some((node) => node.id === nodeId)) {
        menuReturnFocusRef.current = nodeElement.querySelector<HTMLElement>("button") ?? surfaceRef.current;
        setMenu({ kind: "node", x: point.x, y: point.y, nodeId });
        setNodes((ns) => ns.map((node) => ({ ...node, selected: node.id === nodeId })));
        return;
      }
    }
    menuReturnFocusRef.current = surfaceRef.current;
    setMenu({ kind: "pane", x: point.x, y: point.y, clientX, clientY });
  }, [localPoint, setNodes]);

  const handleKeyDown = useCallback(
    (e: KeyboardEvent) => {
      const target = e.target instanceof HTMLElement ? e.target : null;
      // The note and its page menu remain usable beside this nonmodal map.
      // Only keystrokes from the actual map surface belong to its shortcuts.
      // A focused branch also lets go of an Escape that reached no element at
      // all: a window that is not in front has no focused element, and that
      // Escape would otherwise leave the page instead.
      const strayEscape =
        e.key === "Escape" && focusLine !== null && (target === document.body || target === document.documentElement);
      if (!target || (!surfaceRef.current?.contains(target) && !strayEscape) || e.defaultPrevented || e.isComposing) return;
      if (menu && target.closest(".page-canvas-menu")) return;
      // Naming a box or using a native field must never mutate the selection.
      if (target.closest('input, textarea, select, [contenteditable]:not([contenteditable="false"])')) return;

      // Controls outside the nodes keep their native activation and focus keys.
      // In particular, Enter on "Back to note" must not create a sibling box.
      // Node buttons still use Enter/Tab for direct map manipulation.
      if (
        (e.key === "Enter" || e.key === " " || e.key === "Tab") &&
        target?.closest('button, a[href], [role="button"], [role="menuitem"]') &&
        !target.closest(".react-flow__node")
      ) return;

      // Escape and select-all stay available on a read-only map: neither writes.
      //
      // It unwinds one layer at a time — the menu, then the shortcut sheet, then
      // the box being named, then the selection — and only reaches the window
      // handler that leaves the page once the canvas has nothing of its own left
      // to dismiss. Letting it through unconditionally meant closing a menu threw
      // the reader out of the page they were drawing.
      if (e.key === "Escape") {
        if (menu) setMenu(null);
        else if (helpOpen) setHelpOpen(false);
        else if (draft || editingId) {
          setDraft(null);
          setEditingId(null);
        } else if (focusLine) {
          setFocusLine(null);
          clearSelection();
        } else if (nodesRef.current.some((n) => n.selected)) clearSelection();
        else return; // nothing of ours to close: Escape means "leave the page"
        // Claimed only once something was actually closed: the window handler
        // that leaves the page skips a press whose default is already
        // prevented, so preventing it up front swallowed the last Escape.
        e.preventDefault();
        e.stopPropagation();
        return;
      }
      if ((e.metaKey || e.ctrlKey) && (e.key === "a" || e.key === "A")) {
        e.preventDefault();
        selectAll();
        return;
      }
      // Everything below is unmodified, and Cmd/Ctrl belongs to the window
      // rather than to the map: `zoomHotkeysEnabled` in tauri.conf.json gives
      // the webview its own Cmd+plus page zoom, so letting that press reach the
      // zoom keys below would resize the app UI and the map at once. Cmd+Tab and
      // Cmd+Arrow are the system's too, and were being swallowed here.
      // Taking back a change the map made to the page. With nothing of the
      // map's to take back, the press stays the window's.
      if (
        (e.metaKey || e.ctrlKey) && !e.shiftKey && !e.altKey &&
        e.key.toLowerCase() === "z" && !readOnly && undoRef.current.length > 0
      ) {
        e.preventDefault();
        void undoLineEdit();
        return;
      }
      if (e.metaKey || e.ctrlKey) return;

      if (!e.altKey && (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey))) {
        e.preventDefault();
        openKeyboardContextMenu(target);
        return;
      }

      if (e.key.toLowerCase() === "n" && !e.shiftKey && !e.altKey) {
        if (readOnly) return;
        e.preventDefault();
        if (!independentIdeasSupported) {
          setNotice(t("pageCanvas.independentIdeasUnavailable"));
          return;
        }
        if (isEmptyMap) {
          const rect = surfaceRef.current?.getBoundingClientRect();
          openEmptyIdeaAt((rect?.width ?? 320) / 2, (rect?.height ?? 240) / 2);
        } else if (rootId) {
          const rect = surfaceRef.current?.getBoundingClientRect();
          const point = screenToFlowPosition({
            x: (rect?.left ?? 0) + (rect?.width ?? 320) / 2,
            y: (rect?.top ?? 0) + (rect?.height ?? 240) / 2,
          });
          startDraftAt(rootId, point.x - DRAFT_SIZE.width / 2, point.y - DRAFT_SIZE.height / 2);
        }
        return;
      }

      // "?" is what GitHub, Linear and Slack all bind to the shortcut sheet.
      // e.code again: the glyph is Shift+/ on a US layout and something else
      // elsewhere, but the physical key is Slash either way.
      if (e.shiftKey && e.code === "Slash") {
        e.preventDefault();
        setHelpOpen((v) => !v);
        return;
      }
      // Bare +/- rather than the Cmd pair every browser uses, because Cmd is the
      // window's (above) — and bare is what Figma, Miro and tldraw bind, so it
      // is the gesture people arrive with.
      //
      // Matched by glyph AND by physical key, because neither alone is enough:
      // e.code binds the US position of "+", which on a German layout is a
      // different key entirely, so those readers would press the key printed
      // with a + and get nothing; e.key alone misses the numpad on layouts that
      // report it oddly. Shift is not excluded — "+" *is* Shift+Equal here.
      if (e.key === "+" || e.key === "=" || e.code === "Equal" || e.code === "NumpadAdd") {
        e.preventDefault();
        void zoomIn({ duration: ZOOM_DURATION });
        return;
      }
      if (e.key === "-" || e.code === "Minus" || e.code === "NumpadSubtract") {
        e.preventDefault();
        void zoomOut({ duration: ZOOM_DURATION });
        return;
      }
      // Figma's view keys — 0 for actual size, 1 for fit, 2 for fit selection —
      // which is also what the map opens at, so 0 is the way back.
      // e.code rather than e.key: Shift+1 reports "!" on a US layout, and a
      // different glyph again on most others.
      if (e.shiftKey && e.code === "Digit0") {
        e.preventDefault();
        void zoomTo(1, { duration: ZOOM_DURATION });
        return;
      }
      if (e.shiftKey && (e.code === "Digit1" || e.code === "Digit2")) {
        e.preventDefault();
        const picked =
          e.code === "Digit2" ? nodesRef.current.filter((n) => n.selected) : [];
        // Passing `nodes: undefined` is not the same as omitting it: React Flow
        // reads the key as "fit exactly these" and an empty list fits nothing.
        //
        // Zooming to a selection keeps no ceiling: filling the frame with one box
        // is the whole gesture, and Figma's Shift 2 magnifies for the same reason.
        void fitView(
          picked.length ? { duration: 200, nodes: picked } : FIT_ALL_VIEW,
        );
        return;
      }
      // Option 1–9 shows the map down to that level; Option 0 shows all of it.
      // e.code: Option+digit types a symbol on a Mac keyboard.
      if (e.altKey && !e.shiftKey && /^Digit[0-9]$/.test(e.code) && hasPageLines) {
        e.preventDefault();
        const level = Number(e.code.slice(5));
        foldToLevel(level === 0 ? null : level);
        return;
      }
      if (e.key === "/" && !e.altKey) {
        e.preventDefault();
        setSearch((current) => current ?? { query: "", at: -1 });
        return;
      }
      if (readOnly) return;
      if (e.altKey && (e.key === "ArrowUp" || e.key === "ArrowDown")) {
        const picked = nodesRef.current.filter((n) => n.selected);
        if (picked.length === 1 && isPageLine(picked[0].id)) {
          e.preventDefault();
          if (!linesPaused()) moveLineStep(picked[0].id, e.key === "ArrowDown" ? 1 : -1);
          return;
        }
      }
      // Every shortcut below moves, renames or removes something the open menu
      // is pointing at, so the menu stops being about anything.
      if (menu) setMenu(null);

      const selected = nodesRef.current.find((n) => n.selected);
      const step = e.shiftKey ? NUDGE_STEP_COARSE : NUDGE_STEP;
      switch (e.key) {
        case "Tab":
          e.preventDefault();
          startDraft("child");
          break;
        case "Enter":
          e.preventDefault();
          startDraft("sibling");
          break;
        case "F2":
          e.preventDefault();
          if (selected && isPageLine(selected.id)) void startLineRename(selected.id);
          else if (selected) setEditingId(selected.id);
          break;
        case "Delete":
        case "Backspace":
          e.preventDefault();
          deleteSelection();
          break;
        case "ArrowUp":
          e.preventDefault();
          if (!stepSelection(e.key)) nudge(0, -step);
          break;
        case "ArrowDown":
          e.preventDefault();
          if (!stepSelection(e.key)) nudge(0, step);
          break;
        case "ArrowLeft":
          e.preventDefault();
          if (!stepSelection(e.key)) nudge(-step, 0);
          break;
        case "ArrowRight":
          e.preventDefault();
          if (!stepSelection(e.key)) nudge(step, 0);
          break;
        default:
          break;
      }
    },
    [
      readOnly,
      independentIdeasSupported,
      isEmptyMap,
      rootId,
      screenToFlowPosition,
      startDraftAt,
      openEmptyIdeaAt,
      openKeyboardContextMenu,
      startDraft,
      deleteSelection,
      startLineRename,
      undoLineEdit,
      nudge,
      stepSelection,
      selectAll,
      clearSelection,
      fitView,
      zoomIn,
      zoomOut,
      zoomTo,
      menu,
      draft,
      editingId,
      helpOpen,
      focusLine,
      hasPageLines,
      foldToLevel,
      moveLineStep,
      linesPaused,
    ],
  );

  // The array React Flow actually renders. In the common case — including
  // every frame of a drag — this IS the local state, so nothing is rebuilt and
  // memo(CanvasNode) holds for every box.
  const displayNodes = useMemo<Node[]>(() => {
    // First frame with data: state is still empty because the sync effect has
    // not run yet, and mounting React Flow on an empty array would make
    // fitView frame nothing.
    const raw = nodes.length === 0 ? serverNodes : nodes;
    const base = drop
      ? raw.map((n) => (n.id === drop.id ? { ...n, className: `is-drop-${drop.place}` } : n))
      : raw;
    if (!editingId && !draftNode) return base;
    const out = editingId
      ? base.map((n) =>
          n.id === editingId
            ? {
                ...n,
                data: {
                  ...n.data,
                  editing: true,
                  ...(lineRename?.id === n.id
                    ? { label: lineRename.text, nameInputLabel: t("pageCanvas.nodeNameLabel") }
                    : {}),
                },
              }
            : n,
        )
      : base.slice();
    if (draftNode) out.push(draftNode);
    return out;
  }, [nodes, serverNodes, editingId, draftNode, lineRename, t, drop]);

  const rfEdges = useMemo<Edge[]>(() => {
    const rendered = new Set(views.map((v) => v.node.id));
    const byId = new Map(views.map((v) => [v.node.id, v]));
    const edges: Edge[] = [];
    for (const v of views) {
      const parent = v.node.parent_id;
      if (!parent || !rendered.has(parent)) continue;
      const from = byId.get(parent);
      // "tree-" prefix keeps derived spine edges out of the server id space.
      edges.push({
        id: `tree-${v.node.id}`,
        source: parent,
        sourceHandle: "anchor",
        target: v.node.id,
        data: layoutMode === "indent" && from
          ? {
              indent: {
                fromX: from.x - from.width / 2 + 12,
                fromY: from.y + from.height / 2,
                toX: v.x - v.width / 2,
                // Text on a stroke is met at the start of its stroke, not at
                // the middle of the words, or the guide line stops short of it.
                toY: v.y + (onStroke(v) ? v.height / 2 : 0),
              },
            }
          : layoutMode === "tree" && from
            ? { tree: true, ends: treeEnds(from, v) }
            : { tree: false },
        // The lines are the structure, so they thin out as the structure does:
        // spokes off the page carry weight, the twigs past them recede. Drawn in
        // one flat ink they were the faintest thing on a map they hold together.
        ...branchLook(spineLook(v.depth), inkOf(v.node.id), v.depth, isDimmed(v.node.id, false), palette.surface),
      });
    }
    for (const e of map?.edges ?? []) {
      if (e.status === "dismissed") continue;
      if (!rendered.has(e.from_node) || !rendered.has(e.to_node)) continue;
      edges.push({
        id: e.id,
        source: e.from_node,
        sourceHandle: "anchor",
        target: e.to_node,
        label: e.label ?? undefined,
        style: {
          stroke: palette.bridge,
          strokeWidth: 1,
          strokeDasharray: "4 3",
        },
      });
    }
    // A box dragged out of a connector is a child of the box it came from, and
    // the connection line vanishes the moment the pointer lifts. Without this,
    // the new box hangs unattached until the daemon's copy arrives, which looks
    // exactly like the drag having failed.
    const spot = draft ?? pending;
    if (spot && rendered.has(spot.parentId)) {
      edges.push({
        id: `tree-${DRAFT_ID}`,
        source: spot.parentId,
        sourceHandle: "anchor",
        target: DRAFT_ID,
        ...spineLook((views.find((v) => v.node.id === spot.parentId)?.depth ?? 0) + 1),
      });
    }
    return edges;
  }, [views, map?.edges, palette, draft, pending, layoutMode, inkOf, isDimmed]);

  // Capture keeps the map's layered Escape ahead of parent page handlers.
  // The listener is scoped to surface descendants so neighboring note text,
  // page menus, and drawer controls retain their own keyboard behavior.
  useEffect(() => {
    document.addEventListener("keydown", handleKeyDown, true);
    return () => document.removeEventListener("keydown", handleKeyDown, true);
  }, [handleKeyDown]);

  // Focus moves into the map on open so its scoped shortcuts work immediately
  // and a screen reader lands in the region it just asked for.
  useEffect(() => {
    surfaceRef.current?.focus({ preventScroll: true });
  }, [isLoading]);

  // Rendering nothing here made every slow fetch look like a canvas that had
  // simply failed to arrive, with no way to tell waiting from broken.
  // How wide the map has to be before the page's lines grow sideways.
  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) return;
    const measure = () => setNarrow(surface.clientWidth < 640);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(surface);
    return () => observer.disconnect();
  }, [isLoading, views.length === 0]);

  // A page's lines open where reading starts: the page box at the top left,
  // at actual size, rather than the whole tree shrunk to fit. Once per page
  // and shape, so a refetch never moves the view under the reader.
  const framedRef = useRef<string | null>(null);
  useEffect(() => {
    if (layoutMode === "radial" || views.length === 0 || !viewportInitialized) return;
    const surface = surfaceRef.current;
    if (!surface) return;
    const key = `${pageId}:${layoutMode}`;
    if (framedRef.current === key) return;
    framedRef.current = key;
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const v of views) {
      minX = Math.min(minX, v.x - v.width / 2);
      maxX = Math.max(maxX, v.x + v.width / 2);
      minY = Math.min(minY, v.y - v.height / 2);
      maxY = Math.max(maxY, v.y + v.height / 2);
    }
    const pad = 20;
    const w = surface.clientWidth;
    const h = surface.clientHeight;
    const zoom = layoutMode === "indent"
      ? Math.min(1, Math.max(0.75, (w - pad * 2) / (maxX - minX)))
      : Math.min(1, Math.max(0.75, (h - pad * 2) / (maxY - minY)));
    const tall = (maxY - minY) * zoom > h - pad * 2;
    void setViewport({
      x: pad - minX * zoom,
      // Clear of the suggestions button in the top corner.
      y: layoutMode === "indent" || tall ? pad + 52 - minY * zoom : h / 2 - ((minY + maxY) / 2) * zoom,
      zoom,
    });
  }, [layoutMode, views, pageId, setViewport, viewportInitialized]);

  // AI guesses wait behind one visible button and never show a count: the
  // page's own lines are the map, these are extras to take or leave.
  const shownSuggestions = showSuggestions
    ? (map?.nodes ?? []).filter((n) => countsAsSuggestion(n, map)).length
    : 0;
  const noticeView = notice ? (
    <span role="status" aria-live="polite" className="page-canvas-notice">
      {notice}
      {undoOffer && !readOnly && (
        <button type="button" className="page-canvas-notice-undo" onClick={() => void undoLineEdit()}>
          {t("pageCanvas.lineUndo")}
        </button>
      )}
    </span>
  ) : null;
  const relatedActions = readOnly || !map || map.map_schema < 1 ? null : (
    <>
      {!showSuggestions && (
        <button
          type="button"
          className="page-canvas-reveal"
          disabled={improveMutation.isPending}
          onClick={() => (hiddenSuggestions > 0 ? setShowSuggestions(true) : improveMutation.mutate())}
        >
          {improveMutation.isPending ? t("pageCanvas.generating") : t("pageCanvas.addRelated")}
        </button>
      )}
      {showSuggestions && shownSuggestions > 0 && (
        <button
          type="button"
          className="page-canvas-reveal"
          disabled={acceptAllMutation.isPending}
          onClick={() => acceptAllMutation.mutate()}
        >
          {t("pageCanvas.acceptAll")}
        </button>
      )}
      {showSuggestions && (
        <button type="button" className="page-canvas-reveal" onClick={() => setShowSuggestions(false)}>
          {t("pageCanvas.hideRelated")}
        </button>
      )}
    </>
  );

  if (isLoading) {
    return <CanvasMessage title={t("pageCanvas.loading")} live />;
  }

  if (error) {
    const status = asPageMapApiError(error)?.status;
    if (status === 404 || status === 405) {
      return <>
        <CanvasMessage
          title={t("pageCanvas.daemonOutdatedTitle")}
          body={t("pageCanvas.daemonOutdatedBody")}
        />
      </>;
    }
    return <>
      <CanvasMessage
        title={t("pageCanvas.loadErrorTitle")}
        body={t("pageCanvas.loadErrorBody")}
        action={
          <button
            type="button"
            className="page-canvas-message-action"
            onClick={() => void refetch()}
          >
            {t("pageCanvas.retry")}
          </button>
        }
      />
    </>;
  }

  if (views.length === 0) {
    return (
      <><div className="page-canvas">
      <div
        ref={surfaceRef}
        className="page-canvas-surface is-message"
        role="region"
        tabIndex={0}
        aria-label={t("pageCanvas.regionLabel", { title: pageTitle })}
        onPointerDownCapture={() => surfaceRef.current?.focus({ preventScroll: true })}
        onContextMenu={handlePaneContextMenu}
        onDoubleClick={(event) => {
          if (readOnly || !independentIdeasSupported) {
            if (!readOnly) setNotice(t("pageCanvas.independentIdeasUnavailable"));
            return;
          }
          const point = localPoint(event.clientX, event.clientY);
          openEmptyIdeaAt(point.x, point.y);
        }}
      >
          <div className="page-canvas-message">
            <p className="page-canvas-message-title">{t(readOnly ? "pageCanvas.emptyTitle" : "pageCanvas.ideasEmptyTitle")}</p>
            <p className="page-canvas-message-body">{t(readOnly ? "pageCanvas.emptyBody" : "pageCanvas.ideasEmptyBody")}</p>
          </div>
          {ideaEntry}
          <div className="page-canvas-actions">{relatedActions}</div>
          {menu && menuItems.length > 0 && (
            <ContextMenu
              menuRef={menuElementRef}
              x={menu.x}
              y={menu.y}
              items={menuItems}
              onClose={restoreMenuFocus}
            />
          )}
          {noticeView}
          {!readOnly && <CanvasHelp open={helpOpen} setOpen={setHelpOpen} />}
        </div>
      </div></>
    );
  }

  return (
    <div className="page-canvas">
      {readOnly && (
        <p role="status" className="page-canvas-banner">
          {t("pageCanvas.readOnlyNotice")}
        </p>
      )}
      <div
        ref={surfaceRef}
        className="page-canvas-surface"
        role="region"
        // Focusable, or the shortcuts below only fire once a box has been
        // clicked — press Tab on a fresh canvas and nothing happens.
        tabIndex={0}
        aria-label={t("pageCanvas.regionLabel", { title: pageTitle })}
        onDoubleClick={handleDoubleClick}
        onPointerDownCapture={(event) => {
          // Clicking the map again after reading the note restores shortcuts.
          // Native controls and node buttons keep their own browser focus.
          if (event.target instanceof Element && !event.target.closest(
            'button, a[href], input, textarea, select, [role="button"], [role="menuitem"], [contenteditable]:not([contenteditable="false"])',
          )) surfaceRef.current?.focus({ preventScroll: true });
        }}
      >
        <ReactFlow
          nodes={displayNodes}
          edges={rfEdges}
          nodeTypes={nodeTypes}
          edgeTypes={edgeTypes}
          onNodesChange={onNodesChange}
          onNodeDrag={handleNodeDrag}
          onNodeDragStop={handleNodeDragStop}
          onNodeDoubleClick={handleNodeDoubleClick}
          onNodeContextMenu={handleNodeContextMenu}
          onPaneContextMenu={handlePaneContextMenu}
          onPaneClick={() => {
            setMenu(null);
            setNotice(null);
            setFocusLine(null);
          }}
          onNodeClick={() => setMenu(null)}
          onMoveStart={() => setMenu(null)}
          onConnectEnd={handleConnectEnd}
          nodesDraggable={!readOnly}
          nodesConnectable={!readOnly}
          edgesFocusable={false}
          // The Obsidian/Figma pointer model: drag empty canvas to rubber-band
          // a selection, two-finger scroll to pan, middle-drag to pan, and
          // pinch (or Cmd-scroll) to zoom.
          selectionOnDrag
          // Meta alone is the React Flow default; Shift is what someone
          // coming from Obsidian or Figma reaches for first.
          multiSelectionKeyCode={["Meta", "Control", "Shift"]}
          panOnDrag={[1]}
          panOnScroll
          // Double-click is how a box gets drawn here, so it must not also be
          // how the canvas zooms.
          zoomOnDoubleClick={false}
          // Delete is handled above, against the map's revision. Left to React
          // Flow it would drop the box from the canvas locally and leave the
          // daemon's copy untouched.
          deleteKeyCode={null}
          fitView={!hasPageLines}
          fitViewOptions={OPENING_VIEW}
          // Below React Flow's default floor of 0.5, because the legibility
          // floor belongs to the opening view above and nowhere else. Left at
          // 0.5, a 28-box map answered "fit all" by clamping there and leaving
          // ten boxes outside the frame — a command that says it shows
          // everything has to show everything, however small.
          minZoom={0.01}
          // Where the zoom keys are discovered. React Flow puts these strings on
          // both the `title` and the `aria-label` of its own zoom buttons, so
          // hovering the control someone already reached for is what tells them
          // there is a key for it — no second surface to explain the first.
          ariaLabelConfig={{
            "controls.zoomIn.ariaLabel": `${t("pageCanvas.zoomIn")} (+)`,
            "controls.zoomOut.ariaLabel": `${t("pageCanvas.zoomOut")} (-)`,
            "controls.fitView.ariaLabel": `${t("pageCanvas.fitView")} (Shift 1)`,
          }}
          proOptions={{ hideAttribution: true }}
        >
          <Background color={palette.graticule} gap={24} />
          {/* React Flow's fit button calls `fitView` with the options passed
              here, not the ones on <ReactFlow>, so the button has to be handed
              the same object as the key or the two disagree. */}
          <Controls showInteractive={false} fitViewOptions={FIT_ALL_VIEW}>
            {hasPageLines && (
              <ControlButton
                onClick={() => setSearch((current) => (current ? null : { query: "", at: -1 }))}
                aria-label={t("pageCanvas.searchButton")}
                title={t("pageCanvas.searchButton")}
              >
                <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
                  <circle cx="7" cy="7" r="4.5" fill="none" stroke="currentColor" strokeWidth="1.6" />
                  <path d="M10.5 10.5 14 14" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
                </svg>
              </ControlButton>
            )}
          </Controls>
        </ReactFlow>
        {menu && menuItems.length > 0 && (
          <ContextMenu
            menuRef={menuElementRef}
            x={menu.x}
            y={menu.y}
            items={menuItems}
            onClose={restoreMenuFocus}
          />
        )}
        {readOnly && (
          <span className="page-canvas-badge" title={t("pageCanvas.readOnlyNotice")}>
            {t("pageCanvas.readOnlyBadge")}
          </span>
        )}
        {search && (
          <MapSearch
            query={search.query}
            at={search.at}
            total={searchMatches.length}
            onQuery={(query) => setSearch({ query, at: -1 })}
            onStep={searchStep}
            onClose={() => {
              setSearch(null);
              surfaceRef.current?.focus({ preventScroll: true });
            }}
          />
        )}
        <div className="page-canvas-actions">
          {relatedActions}
          {noticeView}
        </div>
        {!readOnly && <CanvasHelp open={helpOpen} setOpen={setHelpOpen} />}
      </div>
    </div>
  );
}

/** Find a line of the page on its map: Enter goes to the next one, Shift Enter back. */
function MapSearch({
  query,
  at,
  total,
  onQuery,
  onStep,
  onClose,
}: {
  query: string;
  at: number;
  total: number;
  onQuery: (query: string) => void;
  onStep: (direction: 1 | -1) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const inputRef = useRef<HTMLInputElement | null>(null);
  useEffect(() => inputRef.current?.focus({ preventScroll: true }), []);
  const count = query.trim()
    ? total === 0
      ? t("pageCanvas.searchNone")
      : at >= 0
        ? t("pageCanvas.searchCount", { at: at + 1, total })
        : String(total)
    : "";
  return (
    <div className="page-canvas-search" role="search">
      <input
        ref={inputRef}
        type="search"
        value={query}
        placeholder={t("pageCanvas.searchPlaceholder")}
        aria-label={t("pageCanvas.searchLabel")}
        onChange={(e) => onQuery(e.target.value)}
        // The map's own keys stay out of the field while typing.
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Escape") {
            e.preventDefault();
            onClose();
          } else if (e.key === "Enter") {
            e.preventDefault();
            onStep(e.shiftKey ? -1 : 1);
          }
        }}
      />
      {count && <span className="page-canvas-search-count" aria-live="polite">{count}</span>}
    </div>
  );
}

function ContextMenu({
  menuRef,
  x,
  y,
  items,
  onClose,
}: {
  menuRef: RefObject<HTMLUListElement | null>;
  x: number;
  y: number;
  items: MenuItem[];
  onClose: (restore?: boolean) => void;
}) {
  const { t } = useTranslation();
  useEffect(() => {
    menuRef.current?.querySelector<HTMLButtonElement>('[role="menuitem"]:not(:disabled)')?.focus();
  }, [menuRef]);
  const handleKeyDown = (event: ReactKeyboardEvent<HTMLUListElement>) => {
    const buttons = Array.from(event.currentTarget.querySelectorAll<HTMLButtonElement>('[role="menuitem"]:not(:disabled)'));
    const index = buttons.indexOf(document.activeElement as HTMLButtonElement);
    if (event.key === "Escape") { event.preventDefault(); event.stopPropagation(); onClose(); return; }
    if (event.key === "Home" || event.key === "End") {
      event.preventDefault(); event.stopPropagation(); buttons[event.key === "Home" ? 0 : buttons.length - 1]?.focus(); return;
    }
    if (["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight"].includes(event.key)) {
      event.preventDefault(); event.stopPropagation();
      const direction = event.key === "ArrowDown" || event.key === "ArrowRight" ? 1 : -1;
      buttons[(index + direction + buttons.length) % buttons.length]?.focus();
      return;
    }
    if (event.key === "Tab") {
      event.preventDefault(); event.stopPropagation();
      const surface = menuRef.current?.closest<HTMLElement>(".page-canvas-surface");
      onClose(false);
      requestAnimationFrame(() => {
        surface?.focus({ preventScroll: true });
      });
    }
  };
  return (
    <ul
      className="page-canvas-menu"
      ref={menuRef}
      role="menu"
      aria-label={t("pageCanvas.menuLabel")}
      style={{ left: x, top: y }}
      // The menu sits inside the canvas surface, so a click on it would
      // otherwise reach the pane underneath and fire the very handlers that
      // close it.
      onContextMenu={(e) => e.preventDefault()}
      onKeyDown={handleKeyDown}
    >
      {items.map((item, i) => (
        <li key={item.key} role="none">
          <button
            type="button"
            role="menuitem"
            tabIndex={-1}
            disabled={item.disabled}
            className={[
              "page-canvas-menu-item",
              item.danger ? "is-danger" : "",
              // A rule above the destructive verb, so the pointer travelling
              // down the list crosses a line before it reaches the one item
              // that takes boxes away and cannot be undone.
              item.danger && !items[i - 1]?.danger ? "is-separated" : "",
            ]
              .filter(Boolean)
              .join(" ")}
            onClick={() => {
              const restore = item.run();
              onClose(restore !== false);
            }}
          >
            {item.label}
          </button>
        </li>
      ))}
    </ul>
  );
}

function CanvasHelp({
  open,
  setOpen,
}: {
  open: boolean;
  setOpen: Dispatch<SetStateAction<boolean>>;
}) {
  const { t } = useTranslation();
  const buttonRef = useRef<HTMLButtonElement | null>(null);
  const helpPanelId = useId();

  return (
    <div
      className="page-canvas-help"
      // Escape is a canvas shortcut too (it clears the selection), so an open
      // panel has to swallow its own dismissal before the surface sees it.
      onKeyDown={(e) => {
        if (e.key !== "Escape" || !open) return;
        e.preventDefault();
        e.stopPropagation();
        setOpen(false);
        buttonRef.current?.focus();
      }}
    >
      {open && (
        <div
          id={helpPanelId}
          className="page-canvas-help-panel"
          role="note"
          aria-label={t("pageCanvas.hintsLabel")}
        >
          {SHORTCUTS.map((s) => (
            <div key={s.cap} className="page-canvas-help-row">
              <kbd>{s.cap}</kbd>
              <span>{t(s.key)}</span>
            </div>
          ))}
          <p className="page-canvas-help-note">{t("pageCanvas.hintSectionNote")}</p>
        </div>
      )}
      <button
        ref={buttonRef}
        type="button"
        className="page-canvas-help-button"
        aria-label={t("pageCanvas.hintsLabel")}
        aria-expanded={open}
        aria-controls={open ? helpPanelId : undefined}
        onClick={() => setOpen((v) => !v)}
      >
        ?
      </button>
    </div>
  );
}

/**
 * Waiting, empty, and failed — all three inside the frame the map would fill.
 *
 * Returned as bare text, they collapsed the page to a few lines floating where a
 * 620px canvas had been, which reads as a broken screen rather than as a canvas
 * that has not arrived.
 */
function CanvasMessage({
  title,
  body,
  action,
  live,
}: {
  title: string;
  body?: string;
  action?: ReactNode;
  /** Announce it: true while waiting, so the wait is not silent. */
  live?: boolean;
}) {
  return (
    <div className="page-canvas">
      <div
        className="page-canvas-surface is-message"
        role={live ? "status" : undefined}
        aria-live={live ? "polite" : undefined}
      >
        <div className="page-canvas-message">
          <p className="page-canvas-message-title">{title}</p>
          {body && <p className="page-canvas-message-body">{body}</p>}
          {action}
        </div>
      </div>
    </div>
  );
}
