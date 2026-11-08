// SPDX-License-Identifier: AGPL-3.0-only
// UX-GRAPH-1: the same point is in the same place every time the map opens.
// The settled body positions (pages and entities; memories ride their anchor)
// are kept per scope, and a later open restores them so only new points are
// placed. Rearranging is the person's choice and keeps the previous layout
// for one undo.

export type BodyPositions = Map<string, { x: number; y: number }>;

export const POSITIONS_STORAGE_PREFIX = "atlas.positions.";
export const PREVIOUS_POSITIONS_STORAGE_PREFIX = "atlas.positions.previous.";

/** A stored layout with more points than this is not kept (it would be a few
 *  MB of local storage); the map then lays out as before. */
const MAX_STORED_POINTS = 20_000;

const keyFor = (prefix: string, scope: string | null) => `${prefix}${scope ?? "*"}`;

/** Two decimals is far below a pixel at any zoom the map allows. */
const round = (value: number) => Math.round(value * 100) / 100;

export function encodePositions(positions: BodyPositions): string {
  const out: Record<string, [number, number]> = {};
  for (const [id, { x, y }] of positions) {
    if (Number.isFinite(x) && Number.isFinite(y)) out[id] = [round(x), round(y)];
  }
  return JSON.stringify(out);
}

export function decodePositions(raw: string | null): BodyPositions {
  const positions: BodyPositions = new Map();
  if (!raw) return positions;
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return positions;
    for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
      if (!Array.isArray(value) || value.length !== 2) continue;
      const [x, y] = value;
      if (typeof x === "number" && typeof y === "number" && Number.isFinite(x) && Number.isFinite(y)) {
        positions.set(id, { x, y });
      }
    }
  } catch {
    // A damaged entry is the same as none: the map lays out fresh.
  }
  return positions;
}

function read(prefix: string, scope: string | null): BodyPositions {
  try {
    return decodePositions(window.localStorage.getItem(keyFor(prefix, scope)));
  } catch {
    return new Map();
  }
}

function write(prefix: string, scope: string | null, positions: BodyPositions | null): void {
  try {
    const key = keyFor(prefix, scope);
    if (!positions || positions.size === 0 || positions.size > MAX_STORED_POINTS) {
      window.localStorage.removeItem(key);
      return;
    }
    window.localStorage.setItem(key, encodePositions(positions));
  } catch {
    // Storage full or unavailable: the map still works, it just lays out fresh.
  }
}

export const readStoredPositions = (scope: string | null) => read(POSITIONS_STORAGE_PREFIX, scope);
export const storePositions = (scope: string | null, positions: BodyPositions) =>
  write(POSITIONS_STORAGE_PREFIX, scope, positions);

/** Forget the kept layout so the next build lays out fresh, keeping it once
 *  so the person can take the rearrangement back. */
export function startRearrange(scope: string | null): void {
  write(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope, read(POSITIONS_STORAGE_PREFIX, scope));
  write(POSITIONS_STORAGE_PREFIX, scope, null);
  writeRegions(PREVIOUS_REGIONS_STORAGE_PREFIX, scope, readRegions(REGIONS_STORAGE_PREFIX, scope));
  writeRegions(REGIONS_STORAGE_PREFIX, scope, null);
}

export const canUndoRearrange = (scope: string | null) => read(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope).size > 0;

/** Put the layout from before the last rearrangement back. */
export function undoRearrange(scope: string | null): void {
  write(POSITIONS_STORAGE_PREFIX, scope, read(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope));
  write(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope, null);
  writeRegions(REGIONS_STORAGE_PREFIX, scope, readRegions(PREVIOUS_REGIONS_STORAGE_PREFIX, scope));
  writeRegions(PREVIOUS_REGIONS_STORAGE_PREFIX, scope, null);
}

/** The undo is offered once; any later choice drops it. */
export function dropRearrangeUndo(scope: string | null): void {
  write(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope, null);
  writeRegions(PREVIOUS_REGIONS_STORAGE_PREFIX, scope, null);
}

// Round 4 (R2): which region each body belongs to is kept per scope beside
// its spot, so opening the map again, or the map growing, never moves a
// point to another region. Only Rearrange works the regions out again.
export const REGIONS_STORAGE_PREFIX = "atlas.regions.";
export const PREVIOUS_REGIONS_STORAGE_PREFIX = "atlas.regions.previous.";
export type BodyRegions = Map<string, string>;

function readRegions(prefix: string, scope: string | null): BodyRegions {
  try {
    const raw = window.localStorage.getItem(keyFor(prefix, scope));
    if (!raw) return new Map();
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return new Map();
    return new Map(Object.entries(parsed as Record<string, unknown>).filter((e): e is [string, string] => typeof e[1] === "string"));
  } catch {
    return new Map();
  }
}

function writeRegions(prefix: string, scope: string | null, regions: BodyRegions | null): void {
  try {
    const key = keyFor(prefix, scope);
    if (!regions || regions.size === 0) window.localStorage.removeItem(key);
    else window.localStorage.setItem(key, JSON.stringify(Object.fromEntries([...regions].slice(0, MAX_STORED_POINTS))));
  } catch {
    // Storage full or unavailable: regions are worked out again next time.
  }
}

export const readStoredRegions = (scope: string | null) => readRegions(REGIONS_STORAGE_PREFIX, scope);
export const readPreviousRegions = (scope: string | null) => readRegions(PREVIOUS_REGIONS_STORAGE_PREFIX, scope);
export const storeRegions = (scope: string | null, regions: BodyRegions) => writeRegions(REGIONS_STORAGE_PREFIX, scope, regions);

// UX-CHAT-5: each night point's spot round its thing's centre is worked out
// once and kept, for every scope (it is relative to the centre, which the
// body positions above keep per scope). A later open puts a kept point back
// exactly there and places only the points it has not seen.
export const NIGHT_OFFSETS_STORAGE_KEY = "atlas.nightOffsets";
/** The night points this viewer has seen on the map: a point not among them
 *  grows in once when it first appears. */
export const NIGHT_SEEN_STORAGE_KEY = "atlas.night.seen";
/** Most kept night entries; the ones on the map now always stay. */
const MAX_NIGHT_ENTRIES = 5_000;

export function readNightOffsets(): BodyPositions {
  try {
    return decodePositions(window.localStorage.getItem(NIGHT_OFFSETS_STORAGE_KEY));
  } catch {
    return new Map();
  }
}

/** Keep `current` (what is on the map now) on top of what was kept before,
 *  dropping the oldest other entries past the cap. */
export function storeNightOffsets(current: BodyPositions): void {
  try {
    const merged: BodyPositions = new Map(current);
    for (const [key, value] of readNightOffsets()) {
      if (merged.size >= MAX_NIGHT_ENTRIES) break;
      if (!merged.has(key)) merged.set(key, value);
    }
    if (merged.size === 0) return;
    window.localStorage.setItem(NIGHT_OFFSETS_STORAGE_KEY, encodePositions(merged));
  } catch {
    // Unavailable storage: points are placed by the same rule next time.
  }
}

/** Things whose cluster was placed while none of their points linked to the
 *  map, so it went to free space beside it. The first time such a thing's
 *  points link to something, its cluster moves once, next to what they link
 *  to, so the link lines stay short (UX-CHAT-5); after that it stays put. */
export const THINGS_APART_STORAGE_KEY = "atlas.thingsApart";

/** The things kept apart, or null when this viewer has no record yet (a map
 *  kept before this record existed): then every kept thing counts as apart,
 *  so one placed before its points linked still moves next to them once. */
export function readThingsApart(): Set<string> | null {
  try {
    const raw = window.localStorage.getItem(THINGS_APART_STORAGE_KEY);
    if (raw === null) return null;
    const parsed = JSON.parse(raw) as unknown;
    return new Set(Array.isArray(parsed) ? parsed.filter((id): id is string => typeof id === "string") : []);
  } catch {
    return new Set();
  }
}

export function storeThingsApart(ids: Iterable<string>): void {
  try {
    const list = [...new Set(ids)].sort().slice(0, MAX_NIGHT_ENTRIES);
    window.localStorage.setItem(THINGS_APART_STORAGE_KEY, JSON.stringify(list));
  } catch {
    // Unavailable storage: a thing placed apart stays where it was.
  }
}

/** The night points seen before, or null when this viewer has no record
 *  (never opened the map, or storage is unavailable). */
export function readSeenNightPoints(): Set<string> | null {
  try {
    const raw = window.localStorage.getItem(NIGHT_SEEN_STORAGE_KEY);
    if (raw === null) return null;
    const parsed = JSON.parse(raw) as unknown;
    return Array.isArray(parsed) ? new Set(parsed.filter((id): id is string => typeof id === "string")) : new Set();
  } catch {
    return null;
  }
}

/** Record `current` as seen, on top of what was seen before. */
export function storeSeenNightPoints(current: Iterable<string>): void {
  try {
    const seen = new Set(current);
    for (const id of readSeenNightPoints() ?? []) {
      if (seen.size >= MAX_NIGHT_ENTRIES) break;
      seen.add(id);
    }
    window.localStorage.setItem(NIGHT_SEEN_STORAGE_KEY, JSON.stringify([...seen]));
  } catch {
    // Unavailable storage: nothing grows next time either (no record).
  }
}

// Round 3: a layout kept from before the balanced map is laid out again, once,
// per scope. The person can put the old one back (the previous slot above).
export const ATLAS_LAYOUT_VERSION = 3;
export const LAYOUT_VERSION_STORAGE_PREFIX = "atlas.layout.version.";

export function readLayoutVersion(scope: string | null): number {
  try {
    const raw = window.localStorage.getItem(keyFor(LAYOUT_VERSION_STORAGE_PREFIX, scope));
    const value = raw === null ? 0 : Number(raw);
    return Number.isFinite(value) ? value : 0;
  } catch {
    return ATLAS_LAYOUT_VERSION;
  }
}

export function storeLayoutVersion(scope: string | null): void {
  try {
    window.localStorage.setItem(keyFor(LAYOUT_VERSION_STORAGE_PREFIX, scope), String(ATLAS_LAYOUT_VERSION));
  } catch {
    // Unavailable storage: the map just does not remember it re-balanced.
  }
}

/** Turn and move `fresh` (no scaling) so the points it shares with `old` sit
 *  as close to their old places as a rigid move allows (orthogonal
 *  Procrustes). With fewer than two shared points it only moves. */
export function alignToKept(fresh: BodyPositions, old: BodyPositions): BodyPositions {
  const shared = [...fresh.keys()].filter((id) => old.has(id));
  if (shared.length === 0) return new Map(fresh);
  let fx = 0, fy = 0, ox = 0, oy = 0;
  for (const id of shared) {
    const f = fresh.get(id)!, o = old.get(id)!;
    fx += f.x; fy += f.y; ox += o.x; oy += o.y;
  }
  fx /= shared.length; fy /= shared.length; ox /= shared.length; oy /= shared.length;
  let sxx = 0, sxy = 0;
  for (const id of shared) {
    const f = fresh.get(id)!, o = old.get(id)!;
    const ax = f.x - fx, ay = f.y - fy, bx = o.x - ox, by = o.y - oy;
    sxx += ax * bx + ay * by;
    sxy += ax * by - ay * bx;
  }
  const angle = shared.length >= 2 ? Math.atan2(sxy, sxx) : 0;
  const cos = Math.cos(angle), sin = Math.sin(angle);
  const out: BodyPositions = new Map();
  for (const [id, { x, y }] of fresh) {
    const ax = x - fx, ay = y - fy;
    out.set(id, { x: ox + ax * cos - ay * sin, y: oy + ax * sin + ay * cos });
  }
  return out;
}
