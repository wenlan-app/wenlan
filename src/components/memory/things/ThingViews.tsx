// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useState, type KeyboardEvent, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { getThingMap, type ThingDetail, type ThingMap, type ThingMapNode, type ThingMemory, type ThingPage } from "../../../lib/tauri";

export type ThingView = "short" | "map";

const VIEW_KEY = "wenlan-thing-view";

function readView(): ThingView {
  try {
    return window.localStorage.getItem(VIEW_KEY) === "map" ? "map" : "short";
  } catch {
    return "short";
  }
}

function writeView(view: ThingView) {
  try {
    window.localStorage.setItem(VIEW_KEY, view);
  } catch {
    // Remembering the view is a convenience; the page works without it.
  }
}

/** Whether a night's steeping (unix seconds) happened today, local time:
 *  then the app says "last night" and marks what it steeped as new. */
export function steepedToday(atSeconds: number, now = new Date()): boolean {
  const at = new Date(atSeconds * 1000);
  return at.getFullYear() === now.getFullYear() && at.getMonth() === now.getMonth() && at.getDate() === now.getDate();
}

/** Oldest first, so the points read in the order they were worked out. */
function pointsOf(thing: ThingDetail): ThingMemory[] {
  return [...(thing.memories ?? [])].sort((a, b) => a.created_at - b.created_at);
}

/**
 * A thing's two views (docs/ux/spec.md, UX-VIEW-1): short text and mind map
 * show the same points; switching does not leave the page, and the last view
 * used is remembered.
 */
export function ThingViews({
  thing,
  meta,
  notice,
  onOpenPage,
}: {
  readonly thing: ThingDetail;
  readonly meta: string;
  /** A quiet line under the views bar, such as a merge proposal. */
  readonly notice?: ReactNode;
  readonly onOpenPage: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const [view, setView] = useState<ThingView>(readView);
  const choose = (next: ThingView) => {
    setView(next);
    writeView(next);
  };
  const points = pointsOf(thing);
  const open = new Set(thing.open_points ?? []);
  // What last night steeped is marked new while the page says "last night".
  const fresh = new Set(thing.dream && steepedToday(thing.dream.at) ? thing.dream.fresh ?? [] : []);
  // The thing's own map (UX-VIEW-7), once something has written it. Until
  // then, and on a daemon without the route, both views fall back to the
  // points grouped by their kind.
  const mapQuery = useQuery({
    queryKey: ["thing-map", thing.id],
    queryFn: () => getThingMap(thing.id),
    retry: false,
  });
  const tree = mapQuery.data ? treeOf(mapQuery.data) : null;
  return (
    <>
      <div className="thing-views-bar">
        <p className="thing-detail-meta">{meta}</p>
        <div className="thing-views-switch" role="group" aria-label={t("thing.viewsLabel")}>
          <button type="button" aria-pressed={view === "short"} onClick={() => choose("short")}>
            {t("thing.viewShort")}
          </button>
          <button type="button" aria-pressed={view === "map"} onClick={() => choose("map")}>
            {t("thing.viewMap")}
          </button>
        </div>
      </div>
      {notice}
      {view === "short" ? (
        <div className="thing-short" data-testid="thing-short">
          {thing.status_line ? <p className="thing-detail-where">{thing.status_line}</p> : null}
          {mapQuery.isPending ? null : tree ? (
            <TreeRecap tree={tree} />
          ) : points.length > 0 ? (
            RECAP_GROUPS.map((group) => (
              <RecapGroup key={group.key} group={group} fresh={fresh} points={points.filter((point) => groupOf(point, open) === group.key)} />
            ))
          ) : thing.status_line ? null : (
            <p className="thing-detail-empty">{t("thing.noPoints")}</p>
          )}
        </div>
      ) : mapQuery.isPending ? null : tree ? (
        <TreeMap tree={tree} />
      ) : (
        <ThingMap thing={thing} points={points} onOpenPage={onOpenPage} />
      )}
    </>
  );
}

type RecapKey = "decided" | "open" | "background";

/** A recap in a fixed order (docs/ux/spec.md, UX-VIEW-6): where it stands and
 *  the next step first, then what is decided, then the background. Each point
 *  already carries its kind, so nothing is rewritten to sort it. "Still open"
 *  joins between decided and background once points can say they are open. */
type RecapLabel = "thing.recapDecided" | "thing.recapOpen" | "thing.recapBackground";

const RECAP_GROUPS: readonly { readonly key: RecapKey; readonly label: RecapLabel }[] = [
  { key: "decided", label: "thing.recapDecided" },
  { key: "open", label: "thing.recapOpen" },
  { key: "background", label: "thing.recapBackground" },
];

const RECAP_SHOWN = 3;

/** `open` holds the points a night's steeping found still open (UX-CHAT-5). */
function groupOf(point: ThingMemory, open: ReadonlySet<string>): RecapKey {
  if (open.has(point.source_id)) return "open";
  switch (point.memory_type) {
    case "decision":
      return "decided";
    default:
      return "background";
  }
}

function RecapGroup({
  group,
  points,
  fresh,
}: {
  readonly group: { readonly key: RecapKey; readonly label: RecapLabel };
  readonly points: readonly ThingMemory[];
  /** Points last night steeped, marked new. */
  readonly fresh: ReadonlySet<string>;
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  if (points.length === 0) return null;
  // Newest first within a group; the rest wait behind one line.
  const newest = [...points].sort((a, b) => b.created_at - a.created_at);
  const shown = open ? newest : newest.slice(0, RECAP_SHOWN);
  const hidden = newest.length - shown.length;
  return (
    <section className="thing-recap-group" aria-label={t(group.label)} data-recap={group.key}>
      <h3>{t(group.label)}</h3>
      <ul>
        {shown.map((point) => (
          <li key={point.source_id} data-fresh={fresh.has(point.source_id) ? "true" : undefined}>{withStop(point.title)}</li>
        ))}
      </ul>
      {hidden > 0 ? (
        <button type="button" className="thing-recap-more" onClick={() => setOpen(true)}>
          {t("thing.recapMore", { count: hidden })}
        </button>
      ) : null}
    </section>
  );
}

/** Ends a point with a full stop in the language around it: `context` is the
 *  words shown before it, such as the question it answers. */
function withStop(text: string, context = ""): string {
  const trimmed = text.trim();
  if (/[.!?。！？…]$/.test(trimmed)) return trimmed;
  return /[⺀-￿]/.test(trimmed + context) ? `${trimmed}。` : `${trimmed}.`;
}

const MAP_WIDTH = 730;
const POINT_WIDTH = 220;
const GROUP_X = 244;
const GROUP_WIDTH = 92;
const CENTER_X = 372;
const CENTER_WIDTH = 150;
const PAGE_X = 556;
const PAGE_WIDTH = MAP_WIDTH - PAGE_X;
const LINE_HEIGHT = 17;
const NODE_PAD_Y = 10;
const NODE_GAP = 10;
const GROUP_GAP = 22;

/** Split a label into at most two lines that fit a node, ending in an ellipsis
 *  when cut. CJK characters count about twice as wide as Latin ones. */
function wrap(text: string, maxUnits = 30): string[] {
  const units = (ch: string) => (/[⺀-￿]/.test(ch) ? 2 : 1);
  const lines: string[] = [];
  let line = "";
  let width = 0;
  const words = /[⺀-￿]/.test(text) ? [...text] : text.split(/(\s+)/);
  for (const word of words) {
    const w = [...word].reduce((sum, ch) => sum + units(ch), 0);
    // Closing punctuation never starts a line; it stays with the word before.
    if (width + w > maxUnits && line.trim() && !/^[，。、；：！？）」』…,.;:!?)]$/.test(word)) {
      lines.push(line.trim());
      if (lines.length === 2) break;
      line = "";
      width = 0;
      if (/^\s+$/.test(word)) continue;
    }
    line += word;
    width += w;
  }
  if (lines.length < 2 && line.trim()) lines.push(line.trim());
  const used = lines.join(" ").replace(/\s+/g, "").length;
  if (used < text.replace(/\s+/g, "").length && lines.length > 0) {
    lines[lines.length - 1] = `${lines[lines.length - 1].replace(/[\s.,;:，。；：]*$/, "")}…`;
  }
  return lines;
}

const nodeHeight = (lines: readonly string[]) => lines.length * LINE_HEIGHT + NODE_PAD_Y * 2;

function NodeText({ x, y, lines, anchor }: { readonly x: number; readonly y: number; readonly lines: readonly string[]; readonly anchor?: "middle" }) {
  return (
    <text x={x} y={y + NODE_PAD_Y + LINE_HEIGHT / 2} dominantBaseline="middle" textAnchor={anchor}>
      {lines.map((line, index) => (
        <tspan key={index} x={x} dy={index === 0 ? 0 : LINE_HEIGHT}>{line}</tspan>
      ))}
    </text>
  );
}

const curve = (x1: number, y1: number, x2: number, y2: number) => {
  const mid = (x1 + x2) / 2;
  return `M${x1} ${y1} C${mid} ${y1} ${mid} ${y2} ${x2} ${y2}`;
};

/** The thing in the middle; on the left its points under the same three kinds
 *  as the short text (newest three each, the rest counted); on the right the
 *  pages it used, which open from the map. */
function ThingMap({
  thing,
  points,
  onOpenPage,
}: {
  readonly thing: ThingDetail;
  readonly points: readonly ThingMemory[];
  readonly onOpenPage: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  // Left side: groups, each a label with its points stacked beside it.
  let y = 0;
  const groups = RECAP_GROUPS.flatMap((group) => {
    const open = new Set(thing.open_points ?? []);
    const all = points.filter((point) => groupOf(point, open) === group.key).sort((a, b) => b.created_at - a.created_at);
    if (all.length === 0) return [];
    const items = all.slice(0, RECAP_SHOWN).map((point) => ({ key: point.source_id, full: point.title, lines: wrap(point.title), more: false }));
    if (all.length > RECAP_SHOWN) {
      const label = t("thing.recapMore", { count: all.length - RECAP_SHOWN });
      items.push({ key: `${group.key}-more`, full: label, lines: [label], more: true });
    }
    const top = y;
    const placed = items.map((item) => {
      const node = { ...item, y, height: nodeHeight(item.lines) };
      y += node.height + NODE_GAP;
      return node;
    });
    const bottom = y - NODE_GAP;
    y += GROUP_GAP - NODE_GAP;
    return [{ key: group.key, label: t(group.label), top, middle: (top + bottom) / 2, items: placed }];
  });
  const leftHeight = Math.max(0, y - GROUP_GAP);
  let py = 0;
  const pages = thing.pages.map((page: ThingPage) => {
    const lines = wrap(page.title, 24);
    const node = { key: page.id, full: page.title, lines, y: py, height: nodeHeight(lines) };
    py += node.height + NODE_GAP;
    return node;
  });
  const rightHeight = Math.max(0, py - NODE_GAP);
  const centerLines = wrap(thing.title ?? t("thing.untitled"), 18);
  const centerHeight = nodeHeight(centerLines) + 4;
  const height = Math.max(leftHeight, rightHeight, centerHeight) + 24;
  const leftTop = (height - leftHeight) / 2;
  const rightTop = (height - rightHeight) / 2;
  const cy = height / 2;
  const open = (pageId: string) => (event: KeyboardEvent<SVGGElement>) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onOpenPage(pageId);
    }
  };
  return (
    <div className="thing-map" data-testid="thing-map">
      <svg viewBox={`0 0 ${MAP_WIDTH} ${height}`} width="100%" role="group" aria-label={t("thing.viewMap")}>
        <g className="thing-map-links" fill="none">
          {groups.map((group) => (
            <g key={group.key}>
              <path d={curve(GROUP_X + GROUP_WIDTH, leftTop + group.middle, CENTER_X, cy)} />
              {group.items.map((item) => (
                <path key={item.key} d={curve(POINT_WIDTH, leftTop + item.y + item.height / 2, GROUP_X, leftTop + group.middle)} />
              ))}
            </g>
          ))}
          {pages.map((page) => (
            <path key={page.key} d={curve(CENTER_X + CENTER_WIDTH, cy, PAGE_X, rightTop + page.y + page.height / 2)} />
          ))}
        </g>
        <g className="thing-map-center">
          <rect x={CENTER_X} y={cy - centerHeight / 2} width={CENTER_WIDTH} height={centerHeight} rx={14} />
          <NodeText x={CENTER_X + CENTER_WIDTH / 2} y={cy - centerHeight / 2 + 2} lines={centerLines} anchor="middle" />
        </g>
        {groups.map((group) => (
          <g key={group.key} role="group" aria-label={group.label}>
            <g className="thing-map-group">
              <rect x={GROUP_X} y={leftTop + group.middle - 15} width={GROUP_WIDTH} height={30} rx={15} />
              <text x={GROUP_X + GROUP_WIDTH / 2} y={leftTop + group.middle} dominantBaseline="middle" textAnchor="middle">{group.label}</text>
            </g>
            {group.items.map((item) => (
              <g key={item.key} className={item.more ? "thing-map-point thing-map-more" : "thing-map-point"}>
                <title>{item.full}</title>
                <rect x={0} y={leftTop + item.y} width={POINT_WIDTH} height={item.height} rx={10} />
                <NodeText x={12} y={leftTop + item.y} lines={item.lines} />
              </g>
            ))}
          </g>
        ))}
        {pages.map((page) => (
          <g
            key={page.key}
            className="thing-map-page"
            role="button"
            tabIndex={0}
            aria-label={page.full}
            onClick={() => onOpenPage(page.key)}
            onKeyDown={open(page.key)}
          >
            <title>{page.full}</title>
            <rect x={PAGE_X} y={rightTop + page.y} width={PAGE_WIDTH} height={page.height} rx={10} />
            <NodeText x={PAGE_X + 12} y={rightTop + page.y} lines={page.lines} />
          </g>
        ))}
      </svg>
    </div>
  );
}

/* ---------- A map the AI wrote from the conversation (UX-VIEW-6, UX-VIEW-7) ---------- */

/** Kinds this app draws in their own way. Any other kind is a plain point, so
 *  the AI can add a shape without breaking the page. */
const KNOWN_KINDS = new Set(["question", "option", "reason", "topic"]);

interface Tree {
  readonly root: ThingMapNode;
  readonly children: ReadonlyMap<string, readonly ThingMapNode[]>;
  readonly byId: ReadonlyMap<string, ThingMapNode>;
}

/** The map as a tree, siblings in rank order; null while it has no points. */
function treeOf(map: ThingMap): Tree | null {
  const root = map.nodes.find((node) => node.parent_id == null);
  if (!root || map.nodes.length < 2) return null;
  const byId = new Map(map.nodes.map((node) => [node.id, node]));
  const children = new Map<string, ThingMapNode[]>();
  for (const node of map.nodes) {
    if (node.parent_id == null || !byId.has(node.parent_id)) continue;
    const list = children.get(node.parent_id) ?? [];
    list.push(node);
    children.set(node.parent_id, list);
  }
  for (const list of children.values()) list.sort((a, b) => a.rank - b.rank);
  return { root, children, byId };
}

const kidsOf = (tree: Tree, node: ThingMapNode) => tree.children.get(node.id) ?? [];

/** Every point under the root in reading order, skipping what was dropped. */
function walk(tree: Tree): ThingMapNode[] {
  const out: ThingMapNode[] = [];
  const visit = (node: ThingMapNode) => {
    for (const child of kidsOf(tree, node)) {
      if (child.state === "dropped") continue;
      out.push(child);
      visit(child);
    }
  };
  visit(tree.root);
  return out;
}

type TreeGroupKey = "decided" | "open" | "background";
type TreeGroupLabel = "thing.recapDecided" | "thing.recapOpen" | "thing.recapBackground";
const TREE_GROUPS: readonly { readonly key: TreeGroupKey; readonly label: TreeGroupLabel }[] = [
  { key: "decided", label: "thing.recapDecided" },
  { key: "open", label: "thing.recapOpen" },
  { key: "background", label: "thing.recapBackground" },
];

/** Decided and open points by their state; the background is every other
 *  point with nothing under it (reasons, facts, a topic's details). Questions
 *  are the map's structure, so they show as a point's context instead. */
function treeGroupOf(tree: Tree, node: ThingMapNode): TreeGroupKey | null {
  if (node.state === "decided") return "decided";
  if (node.state === "open") return "open";
  if (node.kind === "question" || kidsOf(tree, node).length > 0) return null;
  return "background";
}

function TreeRecap({ tree }: { readonly tree: Tree }) {
  const points = walk(tree);
  return (
    <>
      {TREE_GROUPS.map((group) => {
        const items = points.filter((node) => treeGroupOf(tree, node) === group.key);
        return <TreeRecapGroup key={group.key} tree={tree} group={group} items={items} />;
      })}
    </>
  );
}

function TreeRecapGroup({
  tree,
  group,
  items,
}: {
  readonly tree: Tree;
  readonly group: { readonly key: TreeGroupKey; readonly label: TreeGroupLabel };
  readonly items: readonly ThingMapNode[];
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  if (items.length === 0) return null;
  const shown = open ? items : items.slice(0, RECAP_SHOWN);
  const hidden = items.length - shown.length;
  return (
    <section className="thing-recap-group" aria-label={t(group.label)} data-recap={group.key}>
      <h3>{t(group.label)}</h3>
      <ul>
        {shown.map((node) => {
          const parent = node.parent_id ? tree.byId.get(node.parent_id) : undefined;
          const context = parent && parent.id !== tree.root.id ? parent.label : null;
          return (
            <li key={node.id}>
              {context ? <span className="thing-recap-context">{context} </span> : null}
              {withStop(node.label, context ?? "")}
            </li>
          );
        })}
      </ul>
      {hidden > 0 ? (
        <button type="button" className="thing-recap-more" onClick={() => setOpen(true)}>
          {t("thing.recapMore", { count: hidden })}
        </button>
      ) : null}
    </section>
  );
}

const TREE_GAP_X = 28;
const TREE_GAP_Y = 10;
const TREE_LEAF_GAP = 16;

interface PlacedNode {
  readonly node: ThingMapNode;
  readonly depth: number;
  readonly lines: readonly string[];
  readonly y: number;
  readonly height: number;
}

/** A left-to-right tree: the thing on the left, each level a column, leaves
 *  stacked and every parent centred on its children. Its shape is whatever
 *  the AI wrote: questions and options for a decision, a topic tree for
 *  something being learned. */
function TreeMap({ tree }: { readonly tree: Tree }) {
  const { t } = useTranslation();
  let maxDepth = 0;
  const depthOf = (node: ThingMapNode, depth: number) => {
    maxDepth = Math.max(maxDepth, depth);
    for (const child of kidsOf(tree, node)) depthOf(child, depth + 1);
  };
  depthOf(tree.root, 0);
  const column = MAP_WIDTH / (maxDepth + 1);
  const boxWidth = column - TREE_GAP_X;
  const unitsFor = (width: number) => Math.max(10, Math.floor((width - 24) / 7));
  const placed: PlacedNode[] = [];
  let cursor = 0;
  let lastDepth = -1;
  const place = (node: ThingMapNode, depth: number): { top: number; bottom: number; centre: number } => {
    const lines = wrap(node.label, unitsFor(boxWidth));
    const height = nodeHeight(lines);
    const kids = kidsOf(tree, node);
    if (kids.length === 0) {
      if (lastDepth >= 0) cursor += depth < lastDepth ? TREE_LEAF_GAP : TREE_GAP_Y;
      lastDepth = depth;
      const y = cursor;
      cursor += height;
      placed.push({ node, depth, lines, y, height });
      return { top: y, bottom: y + height, centre: y + height / 2 };
    }
    const spans = kids.map((child) => place(child, depth + 1));
    const centre = (spans[0].centre + spans[spans.length - 1].centre) / 2;
    const y = centre - height / 2;
    placed.push({ node, depth, lines, y, height });
    return { top: Math.min(y, spans[0].top), bottom: Math.max(y + height, spans[spans.length - 1].bottom), centre };
  };
  const span = place(tree.root, 0);
  const shift = 12 - Math.min(0, span.top);
  const height = span.bottom - Math.min(0, span.top) + 24;
  const at = new Map(placed.map((item) => [item.node.id, item]));
  const xOf = (depth: number) => depth * column;
  return (
    <div className="thing-map" data-testid="thing-map">
      <svg viewBox={`0 0 ${MAP_WIDTH} ${height}`} width="100%" role="group" aria-label={t("thing.viewMap")}>
        <g className="thing-map-links" fill="none">
          {placed
            .filter((item) => item.node.parent_id && at.has(item.node.parent_id))
            .map((item) => {
              const parent = at.get(item.node.parent_id as string) as PlacedNode;
              return (
                <path
                  key={item.node.id}
                  d={curve(xOf(parent.depth) + boxWidth, shift + parent.y + parent.height / 2, xOf(item.depth), shift + item.y + item.height / 2)}
                />
              );
            })}
        </g>
        {placed.map((item) => {
          const { node } = item;
          const isRoot = node.id === tree.root.id;
          const kind = isRoot ? "root" : KNOWN_KINDS.has(node.kind) ? node.kind : "plain";
          const state = node.state ?? "none";
          const lines = node.state === "decided" && !isRoot ? [...item.lines.slice(0, -1), `${item.lines[item.lines.length - 1]} ✓`] : item.lines;
          return (
            <g key={node.id} className="thing-tree-node" data-kind={kind} data-state={state}>
              <title>{node.label}</title>
              {kind === "reason" ? null : (
                <rect x={xOf(item.depth)} y={shift + item.y} width={boxWidth} height={item.height} rx={isRoot ? 14 : 10} />
              )}
              <NodeText x={xOf(item.depth) + (kind === "reason" ? 2 : 12)} y={shift + item.y} lines={lines} />
            </g>
          );
        })}
      </svg>
    </div>
  );
}
