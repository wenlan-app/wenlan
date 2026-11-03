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
}

export const canUndoRearrange = (scope: string | null) => read(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope).size > 0;

/** Put the layout from before the last rearrangement back. */
export function undoRearrange(scope: string | null): void {
  write(POSITIONS_STORAGE_PREFIX, scope, read(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope));
  write(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope, null);
}

/** The undo is offered once; any later choice drops it. */
export const dropRearrangeUndo = (scope: string | null) => write(PREVIOUS_POSITIONS_STORAGE_PREFIX, scope, null);
