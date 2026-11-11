// SPDX-License-Identifier: AGPL-3.0-only
//
// The page's own lines on its map.
//
// A mind map opens on what the page already says: the daemon reads the page's
// headings and the items under them on every map read (`PageMap.outline`).
// They are the page, drawn as a tree, so they are never stored, never
// suggestions and never need accepting. Here they become read-only boxes that
// join the stored map under its root, ahead of anything the person added.

import type { PageMap, PageMapNode, PageOutlineNode } from "../tauri";

/** Every box drawn from the page body carries this id prefix. */
export const PAGE_LINE_PREFIX = "outline:";
/** The page box used when the map has nothing stored yet. */
export const PAGE_LINE_ROOT = `${PAGE_LINE_PREFIX}root`;
const MORE_PREFIX = `${PAGE_LINE_PREFIX}more:`;

/** A heading longer than this many items opens folded. */
const FOLD_OVER = 3;
/** How many items a folded heading still shows. */
const FOLDED_SHOWS = 2;
/**
 * How many levels under the page open drawn. A heading at this level keeps
 * whatever sits under it folded, so a deeply nested page still opens as a
 * short tree.
 */
const OPEN_DEPTH = 3;

/** Each line's level under the page: 1 for a line hanging off the page. */
function depths(outline: readonly PageOutlineNode[]): Map<string, number> {
  const out = new Map<string, number>();
  for (const line of outline) {
    // Parents always come before their children in reading order.
    out.set(line.id, line.parent_id ? (out.get(line.parent_id) ?? 0) + 1 : 1);
  }
  return out;
}

/** Headings at the deepest drawn level that have lines under them. */
function deepHeadings(outline: readonly PageOutlineNode[]): Set<string> {
  const depth = depths(outline);
  const parents = new Set(outline.map((l) => l.parent_id).filter((p): p is string => p !== null));
  return new Set(
    outline
      .filter((l) => l.kind === "heading" && (depth.get(l.id) ?? 0) >= OPEN_DEPTH && parents.has(l.id))
      .map((l) => l.id),
  );
}

export function isPageLine(id: string): boolean {
  return id.startsWith(PAGE_LINE_PREFIX);
}

/** The heading a "show the rest" box belongs to, or null. */
export function moreFor(id: string): string | null {
  return id.startsWith(MORE_PREFIX) ? id.slice(MORE_PREFIX.length) : null;
}

/** The outline line behind a page-line box, or null for the root and "more" boxes. */
export function outlineLine(map: PageMap | undefined, id: string): PageOutlineNode | null {
  if (!isPageLine(id) || moreFor(id) !== null || id === PAGE_LINE_ROOT) return null;
  const raw = id.slice(PAGE_LINE_PREFIX.length);
  return map?.outline?.find((line) => line.id === raw) ?? null;
}

function box(
  id: string,
  parentId: string | null,
  rank: number,
  refKind: PageMapNode["ref_kind"],
  refId: string,
  label: string,
): PageMapNode {
  return {
    id,
    parent_id: parentId,
    rank,
    ref_kind: refKind,
    ref_id: refId,
    label,
    status: "active",
    pinned: false,
    placed: false,
    collapsed: false,
    x: null,
    y: null,
    width: null,
    height: null,
    ref_state: "live",
  };
}

export interface PageLinesOptions {
  pageId: string;
  pageTitle: string;
  /** Headings the person unfolded. */
  open: ReadonlySet<string>;
  /** Headings the person shut: nothing under them is drawn. */
  closed?: ReadonlySet<string>;
  showSuggestions: boolean;
  showRestLabel: string;
}

/**
 * The nodes the canvas lays out: the stored map (without the old stored
 * `section` boxes when the page's own headings replace them, and without hidden
 * suggestions) plus the page's lines.
 */
export function withPageLines(map: PageMap | undefined, options: PageLinesOptions): PageMapNode[] {
  const outline = map?.outline ?? [];
  const stored = (map?.nodes ?? []).filter(
    (n) =>
      n.status !== "dismissed" &&
      // The page's own headings replace the old stored section boxes, but
      // an older daemon sends no outline, so keep them there.
      (outline.length === 0 || n.ref_kind !== "section") &&
      (options.showSuggestions || n.status !== "suggested"),
  );
  if (outline.length === 0) return stored;

  const storedRoot = stored.find((n) => n.parent_id === null);
  const rootId = storedRoot?.id ?? PAGE_LINE_ROOT;
  const out = storedRoot
    ? stored.slice()
    : [box(PAGE_LINE_ROOT, null, 0, "page", options.pageId, options.pageTitle)];

  const itemsOf = new Map<string | null, PageOutlineNode[]>();
  for (const line of outline) {
    if (line.kind !== "item") continue;
    const bucket = itemsOf.get(line.parent_id);
    if (bucket) bucket.push(line);
    else itemsOf.set(line.parent_id, [line]);
  }
  const shown = new Set<string>();
  for (const [parent, items] of itemsOf) {
    const folded = parent !== null && items.length > FOLD_OVER && !options.open.has(parent);
    for (const item of folded ? items.slice(0, FOLDED_SHOWS) : items) shown.add(item.id);
  }
  // Too many levels: a deep heading stays shut until opened, and so does
  // everything under it.
  const deep = deepHeadings(outline);
  const closed = options.closed ?? new Set<string>();
  const hidden = new Set<string>();
  for (const line of outline) {
    const parent = line.parent_id;
    if (
      parent !== null &&
      (hidden.has(parent) || closed.has(parent) || (deep.has(parent) && !options.open.has(parent)))
    ) {
      hidden.add(line.id);
    }
  }

  // Page lines come first, in reading order, ahead of the person's own boxes.
  const base = -1_000_000;
  outline.forEach((line, index) => {
    if (hidden.has(line.id)) return;
    if (line.kind === "item" && !shown.has(line.id)) return;
    out.push(
      box(
        `${PAGE_LINE_PREFIX}${line.id}`,
        line.parent_id ? `${PAGE_LINE_PREFIX}${line.parent_id}` : rootId,
        base + index,
        line.kind === "heading" ? "section" : "outline",
        `${options.pageId}#${line.anchor ?? line.id}`,
        line.label,
      ),
    );
  });
  for (const id of deep) {
    if (options.open.has(id) || hidden.has(id) || closed.has(id)) continue;
    out.push(
      box(
        `${MORE_PREFIX}${id}`,
        `${PAGE_LINE_PREFIX}${id}`,
        base + outline.findIndex((l) => l.id === id) + 0.5,
        "outline",
        `${options.pageId}#more`,
        options.showRestLabel,
      ),
    );
  }
  for (const [parent, items] of itemsOf) {
    if (parent === null || options.open.has(parent) || items.length <= FOLD_OVER) continue;
    if (deep.has(parent) || hidden.has(parent) || closed.has(parent)) continue;
    const lastShown = outline.indexOf(items[FOLDED_SHOWS - 1]);
    out.push(
      box(
        `${MORE_PREFIX}${parent}`,
        `${PAGE_LINE_PREFIX}${parent}`,
        base + lastShown + 0.5,
        "outline",
        `${options.pageId}#more`,
        options.showRestLabel,
      ),
    );
  }
  return out;
}

/** Headings that fold (long, or too deep), so the box can show a toggle. */
export function foldableHeadings(map: PageMap | undefined): Set<string> {
  const counts = new Map<string, number>();
  for (const line of map?.outline ?? []) {
    if (line.kind === "item" && line.parent_id) {
      counts.set(line.parent_id, (counts.get(line.parent_id) ?? 0) + 1);
    }
  }
  const long = [...counts].filter(([, n]) => n > FOLD_OVER).map(([id]) => id);
  return new Set([...long, ...deepHeadings(map?.outline ?? [])]);
}

/** Headings with anything under them: each can be shut and opened again. */
export function branchHeadings(map: PageMap | undefined): Set<string> {
  const parents = new Set((map?.outline ?? []).map((l) => l.parent_id));
  return new Set((map?.outline ?? []).filter((l) => l.kind === "heading" && parents.has(l.id)).map((l) => l.id));
}

/** Headings too deep to open drawn: they start shut until opened. */
export function shutByDepth(map: PageMap | undefined): Set<string> {
  return deepHeadings(map?.outline ?? []);
}

/** Inks a top-level heading's whole branch can wear, in turn. */
const BRANCH_SLOTS = ["tool", "concept", "person", "org", "page"] as const;
export type BranchSlot = (typeof BRANCH_SLOTS)[number];

/**
 * The ink each line wears: every top-level heading takes the next slot in
 * turn, and everything under it shares that slot, so a long page reads as a
 * few colored branches instead of one grey block. Lines hanging directly off
 * the page that are not headings wear none.
 */
export function branchSlots(map: PageMap | undefined): Map<string, BranchSlot> {
  const out = new Map<string, BranchSlot>();
  let next = 0;
  for (const line of map?.outline ?? []) {
    if (line.parent_id === null) {
      if (line.kind === "heading") out.set(line.id, BRANCH_SLOTS[next++ % BRANCH_SLOTS.length]);
      continue;
    }
    const slot = out.get(line.parent_id);
    if (slot) out.set(line.id, slot);
  }
  return out;
}

/** Whether `lineId` is `focusId` or sits anywhere under it. */
export function withinLine(map: PageMap | undefined, focusId: string, lineId: string): boolean {
  const parentOf = new Map((map?.outline ?? []).map((l) => [l.id, l.parent_id]));
  let at: string | null | undefined = lineId;
  while (at) {
    if (at === focusId) return true;
    at = parentOf.get(at);
  }
  return false;
}

/** The outline line a page-line box stands for, "show the rest" boxes included. */
export function lineIdOf(id: string): string | null {
  if (!isPageLine(id) || id === PAGE_LINE_ROOT) return null;
  return moreFor(id) ?? id.slice(PAGE_LINE_PREFIX.length);
}

/** Each page line's level under the page: 1 for a line hanging off the page. */
export function lineDepths(map: PageMap | undefined): Map<string, number> {
  return depths(map?.outline ?? []);
}

/** The headings a page line hangs on, nearest first. */
export function lineAncestors(map: PageMap | undefined, id: string): string[] {
  const byId = new Map((map?.outline ?? []).map((l) => [l.id, l]));
  const out: string[] = [];
  let at = byId.get(id)?.parent_id ?? null;
  while (at !== null && !out.includes(at)) {
    out.push(at);
    at = byId.get(at)?.parent_id ?? null;
  }
  return out;
}
