// SPDX-License-Identifier: AGPL-3.0-only
//
// The page's own lines on its map.
//
// A mind map opens on what the page already says: the daemon reads the page's
// headings and the items under them on every map read (`PageMap.outline`).
// They are the page, drawn as a tree, so they are never stored, never
// suggestions and never need accepting. Here they become read-only boxes that
// join the stored map under its root, ahead of anything the person added.

import type { PageMap, PageMapNode, PageOutlineLink, PageOutlineNode } from "../tauri";

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

/** Boxes grown from another page sit under this prefix, never in the stored map. */
export const DOOR_IN = `${PAGE_LINE_PREFIX}in:`;
const DOOR_EXTRA = `${PAGE_LINE_PREFIX}door:`;

export type BoxKind =
  /** A link to a page: opens in place. */
  | "door"
  /** The same page again: grey, jumps to its door. */
  | "dup"
  /** A link to the centre page itself. */
  | "back"
  /** A heading of an opened door's page. */
  | "foreignHeading"
  | "foreignItem"
  | "foreignMore"
  /** A link inside an opened door: grey, one hop only. */
  | "foreignGhost"
  | "foreignBack";

/** What a link, door or foreign box stands for. */
export interface BoxMeta {
  kind: BoxKind;
  /** The line of the centre page this box hangs on, for its ink and focus. */
  ownerLineId: string | null;
  /** The shown title of the page behind a door, ghost or copy. */
  title: string;
  /** The page behind a door, ghost or copy. */
  pageId?: string;
  /** The target's Space, when it is another Space than the page's. */
  space?: string;
  /** The door a copy points to, or the door a foreign box grew from. */
  doorId?: string;
  /** The line of an opened door's page this box shows. */
  foreign?: { pageId: string; line: PageOutlineNode; parent: PageOutlineNode | null; hasItems: boolean };
}

export interface DoorOptions {
  /** The centre page: a link to it is drawn as a way back, never a door. */
  centreId: string;
  /** Doors the person opened. */
  open: ReadonlySet<string>;
  /** The pages behind opened doors, by door id; absent while loading. */
  foreign: ReadonlyMap<string, PageMap | undefined>;
  /** Headings of opened pages whose items show, and their "show the rest". */
  foreignOpen: ReadonlySet<string>;
  /** Label of a way back to the centre. */
  backLabel: string;
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
  /** Turns resolved `[[links]]` into doors. Without it links stay plain text. */
  doors?: DoorOptions;
}

/** A link of the centre page that reaches a page, in reading order. */
interface LinkSlot {
  boxId: string;
  link: PageOutlineLink & { page_id: string };
  owner: PageOutlineNode;
  /** The heading it hangs under; null for a line hanging off the page. */
  parentLineId: string | null;
  /** The item's own box is the door. */
  wholeLine: boolean;
  kind: "door" | "dup" | "back";
  doorId?: string;
}

function linkSlots(outline: readonly PageOutlineNode[], centreId: string): LinkSlot[] {
  const out: LinkSlot[] = [];
  const first = new Map<string, string>();
  for (const line of outline) {
    (line.links ?? []).forEach((link, i) => {
      if (!link.page_id) return;
      const wholeLine = line.kind === "item" && link.whole_line === true;
      const boxId = wholeLine ? `${PAGE_LINE_PREFIX}${line.id}` : `${DOOR_EXTRA}${line.id}:${i}`;
      let kind: LinkSlot["kind"] = "door";
      let doorId: string | undefined;
      if (link.page_id === centreId) kind = "back";
      else if (first.has(link.page_id)) {
        kind = "dup";
        doorId = first.get(link.page_id);
      } else first.set(link.page_id, boxId);
      out.push({
        boxId,
        link: link as LinkSlot["link"],
        owner: line,
        parentLineId: line.kind === "heading" ? line.id : line.parent_id,
        wholeLine,
        kind,
        doorId,
      });
    });
  }
  return out;
}

/** Every door of a map in reading order: its box id and the page it leads to. */
export function doorTargets(map: PageMap | undefined, centreId: string): Map<string, string> {
  return new Map(
    linkSlots(map?.outline ?? [], centreId)
      .filter((slot) => slot.kind === "door")
      .map((slot) => [slot.boxId, slot.link.page_id]),
  );
}

/** The line a door was written in, so a hidden door can be brought into view. */
export function doorSlot(
  map: PageMap | undefined,
  centreId: string,
  boxId: string,
): { ownerId: string; ownerIsHeading: boolean } | null {
  const slot = linkSlots(map?.outline ?? [], centreId).find((s) => s.boxId === boxId);
  return slot ? { ownerId: slot.owner.id, ownerIsHeading: slot.owner.kind === "heading" } : null;
}

/** Items of a heading that show before "show the rest". */
function foreignItemsShown(items: readonly PageOutlineNode[], restOpen: boolean): PageOutlineNode[] {
  return items.length > FOLD_OVER && !restOpen ? items.slice(0, FOLDED_SHOWS) : items.slice();
}

/**
 * The nodes the canvas lays out: the stored map (without the old stored
 * `section` boxes when the page's own headings replace them, and without hidden
 * suggestions) plus the page's lines.
 */
export function withPageLines(map: PageMap | undefined, options: PageLinesOptions): PageMapNode[] {
  return withPageLinesDetailed(map, options).nodes;
}

/** `withPageLines`, plus what each door box stands for. */
export function withPageLinesDetailed(
  map: PageMap | undefined,
  options: PageLinesOptions,
): { nodes: PageMapNode[]; meta: Map<string, BoxMeta> } {
  const meta = new Map<string, BoxMeta>();
  const outline = map?.outline ?? [];
  const stored = (map?.nodes ?? []).filter(
    (n) =>
      n.status !== "dismissed" &&
      // The page's own headings replace the old stored section boxes, but
      // an older daemon sends no outline, so keep them there.
      (outline.length === 0 || n.ref_kind !== "section") &&
      (options.showSuggestions || n.status !== "suggested"),
  );
  if (outline.length === 0) return { nodes: stored, meta };

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
  const doors = options.doors;
  const slots = doors ? linkSlots(outline, doors.centreId) : [];
  const wholeLineSlots = new Map(slots.filter((slot) => slot.wholeLine).map((slot) => [slot.boxId, slot]));
  const slotLabel = (slot: LinkSlot) =>
    slot.kind === "back" ? `↩ ${doors?.backLabel ?? ""}` : slot.kind === "dup" ? `${slot.link.title} ↗` : slot.link.title;
  const slotMeta = (slot: LinkSlot): BoxMeta => ({
    kind: slot.kind,
    ownerLineId: slot.owner.id,
    title: slot.link.title,
    pageId: slot.link.page_id,
    space: slot.link.space,
    doorId: slot.doorId,
  });
  outline.forEach((line, index) => {
    if (hidden.has(line.id)) return;
    if (line.kind === "item" && !shown.has(line.id)) return;
    const id = `${PAGE_LINE_PREFIX}${line.id}`;
    // An item that is only a link is the door itself, and keeps its line.
    const door = wholeLineSlots.get(id);
    if (door) meta.set(id, slotMeta(door));
    out.push(
      box(
        id,
        line.parent_id ? `${PAGE_LINE_PREFIX}${line.parent_id}` : rootId,
        base + index,
        line.kind === "heading" ? "section" : "outline",
        `${options.pageId}#${line.anchor ?? line.id}`,
        door ? slotLabel(door) : line.label,
      ),
    );
  });
  // A closed heading (shut by depth or by the person) shows only its toggle:
  // no "show the rest" box, which would only open it one level too.
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
  if (doors) addDoors(out, meta, { outline, slots, rootId, base, closed, open: options.open, deep, hidden, doors, slotLabel, slotMeta, showRestLabel: options.showRestLabel });
  return { nodes: out, meta };
}

interface DoorContext {
  outline: readonly PageOutlineNode[];
  slots: LinkSlot[];
  rootId: string;
  base: number;
  closed: ReadonlySet<string>;
  open: ReadonlySet<string>;
  deep: ReadonlySet<string>;
  hidden: ReadonlySet<string>;
  doors: DoorOptions;
  slotLabel: (slot: LinkSlot) => string;
  slotMeta: (slot: LinkSlot) => BoxMeta;
  showRestLabel: string;
}

/** Door boxes for links written inside text, and the pages opened from doors. */
function addDoors(out: PageMapNode[], meta: Map<string, BoxMeta>, c: DoorContext): void {
  const { outline, base, doors } = c;
  const drawn = new Set(out.map((n) => n.id));
  const lastChild = (parentLineId: string | null): number => {
    let at = parentLineId === null ? -1 : outline.findIndex((l) => l.id === parentLineId);
    outline.forEach((l, i) => {
      if (l.parent_id === parentLineId) at = Math.max(at, i);
    });
    return at;
  };
  const placed = new Map<string | null, number>();
  for (const slot of c.slots) {
    if (slot.wholeLine) continue;
    const parent = slot.parentLineId;
    const parentBox = parent === null ? c.rootId : `${PAGE_LINE_PREFIX}${parent}`;
    if (parent !== null) {
      const shut = c.closed.has(parent) || (c.deep.has(parent) && !c.open.has(parent));
      if (c.hidden.has(parent) || shut || !drawn.has(parentBox)) continue;
    }
    const n = placed.get(parent) ?? 0;
    placed.set(parent, n + 1);
    meta.set(slot.boxId, c.slotMeta(slot));
    out.push(box(slot.boxId, parentBox, base + lastChild(parent) + 0.7 + n * 0.001, "outline", `${slot.link.page_id}#door`, c.slotLabel(slot)));
    drawn.add(slot.boxId);
  }

  for (const slot of c.slots) {
    if (slot.kind !== "door" || !doors.open.has(slot.boxId) || !drawn.has(slot.boxId)) continue;
    const fmap = doors.foreign.get(slot.boxId);
    if (!fmap) continue;
    const fo = fmap.outline ?? [];
    const prefix = `${DOOR_IN}${slot.boxId.slice(PAGE_LINE_PREFIX.length)}|`;
    const ownerLineId = slot.owner.id;
    const ghost = (parentBox: string, link: PageOutlineLink, rank: number, tag: string) => {
      if (!link.page_id) return;
      const back = link.page_id === doors.centreId;
      const id = `${prefix}${tag}`;
      meta.set(id, {
        kind: back ? "foreignBack" : "foreignGhost",
        ownerLineId,
        title: link.title,
        pageId: link.page_id,
        space: link.space,
        doorId: slot.boxId,
      });
      out.push(box(id, parentBox, rank, "outline", `${fmap.page_id}#link`, back ? `↩ ${doors.backLabel}` : `${link.title} →`));
    };
    const itemsBy = new Map<string, PageOutlineNode[]>();
    for (const line of fo) {
      if (line.kind !== "item" || !line.parent_id) continue;
      const bucket = itemsBy.get(line.parent_id);
      if (bucket) bucket.push(line);
      else itemsBy.set(line.parent_id, [line]);
    }
    fo.forEach((line, index) => {
      if (line.kind !== "heading") return;
      const hid = `${prefix}${line.id}`;
      const parentLine = line.parent_id ? (fo.find((l) => l.id === line.parent_id) ?? null) : null;
      const items = itemsBy.get(line.id) ?? [];
      meta.set(hid, {
        kind: "foreignHeading",
        ownerLineId,
        title: line.label,
        doorId: slot.boxId,
        foreign: { pageId: fmap.page_id, line, parent: parentLine, hasItems: items.length > 0 },
      });
      out.push(box(hid, line.parent_id ? `${prefix}${line.parent_id}` : slot.boxId, index, "outline", `${fmap.page_id}#${line.anchor ?? line.id}`, line.label));
      (line.links ?? []).forEach((link, i) => ghost(hid, link, index + 0.7 + i * 0.001, `${line.id}:g${i}`));
      if (!doors.foreignOpen.has(hid)) return;
      const restOpen = doors.foreignOpen.has(`${hid}#all`);
      const shownItems = foreignItemsShown(items, restOpen);
      let extra = 0;
      for (const item of shownItems) {
        const at = fo.indexOf(item);
        const whole = (item.links ?? []).find((l) => l.whole_line && l.page_id);
        if (whole) ghost(hid, whole, at, item.id);
        else {
          const iid = `${prefix}${item.id}`;
          meta.set(iid, {
            kind: "foreignItem",
            ownerLineId,
            title: item.label,
            doorId: slot.boxId,
            foreign: { pageId: fmap.page_id, line: item, parent: line, hasItems: false },
          });
          out.push(box(iid, hid, at, "outline", `${fmap.page_id}#${item.id}`, item.label));
          for (const link of item.links ?? []) ghost(hid, link, index + 0.8 + extra++ * 0.001, `${item.id}:g${extra}`);
        }
      }
      if (shownItems.length < items.length) {
        const mid = `${prefix}more:${line.id}`;
        meta.set(mid, { kind: "foreignMore", ownerLineId, title: line.label, doorId: slot.boxId, foreign: { pageId: fmap.page_id, line, parent: parentLine, hasItems: true } });
        out.push(box(mid, hid, fo.indexOf(shownItems[shownItems.length - 1]) + 0.5, "outline", `${fmap.page_id}#more`, c.showRestLabel));
      }
    });
  }
  void base;
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
