// SPDX-License-Identifier: AGPL-3.0-only
import type { RecentPageEntry } from "./recentPages";

export type WikiInventoryMode = "recent" | "custom" | "folders" | "archived";
export type WikiNotesStorage = {
  readonly getItem: (key: string) => string | null;
  readonly setItem: (key: string, value: string) => void;
};

const STORAGE_KEY = "wenlan:wiki-inventory:v1";
const VERSION = 1 as const;
const LIMIT = 50;
const MODES = new Set<WikiInventoryMode>(["recent", "custom", "folders", "archived"]);
type StoredPreferences = { readonly version: 1; readonly mode: WikiInventoryMode; readonly customOrder: readonly string[] };

function resolveStorage(storage?: WikiNotesStorage | null): WikiNotesStorage | null {
  if (storage !== undefined) return storage;
  try { return globalThis.localStorage; } catch { return null; }
}

function readPreferences(storage?: WikiNotesStorage | null): StoredPreferences {
  const resolved = resolveStorage(storage);
  if (resolved === null) return { version: VERSION, mode: "recent", customOrder: [] };
  try {
    const raw = resolved.getItem(STORAGE_KEY);
    if (raw === null) return { version: VERSION, mode: "recent", customOrder: [] };
    const value: unknown = JSON.parse(raw);
    if (typeof value !== "object" || value === null || Array.isArray(value)) return { version: VERSION, mode: "recent", customOrder: [] };
    const record = value as Record<string, unknown>;
    const mode = MODES.has(record["mode"] as WikiInventoryMode) ? record["mode"] as WikiInventoryMode : "recent";
    const customOrder = Array.isArray(record["customOrder"])
      ? [...new Set(record["customOrder"].filter((id): id is string => typeof id === "string" && id.length > 0))].slice(0, LIMIT)
      : [];
    return record["version"] === VERSION ? { version: VERSION, mode, customOrder } : { version: VERSION, mode: "recent", customOrder: [] };
  } catch {
    return { version: VERSION, mode: "recent", customOrder: [] };
  }
}

function writePreferences(preferences: StoredPreferences, storage?: WikiNotesStorage | null): void {
  const resolved = resolveStorage(storage);
  if (resolved === null) return;
  try { resolved.setItem(STORAGE_KEY, JSON.stringify(preferences)); } catch { /* Preferences are optional; callers keep in-session state. */ }
}

export function readWikiInventoryMode(storage?: WikiNotesStorage | null): WikiInventoryMode {
  return readPreferences(storage).mode;
}

export function writeWikiInventoryMode(mode: WikiInventoryMode, storage?: WikiNotesStorage | null): void {
  if (!MODES.has(mode)) return;
  const current = readPreferences(storage);
  writePreferences({ ...current, mode }, storage);
}

export function readWikiCustomOrder(storage?: WikiNotesStorage | null): readonly string[] {
  return readPreferences(storage).customOrder;
}

export function writeWikiCustomOrder(ids: readonly string[], storage?: WikiNotesStorage | null): void {
  const current = readPreferences(storage);
  const customOrder = [...new Set(ids.filter(id => typeof id === "string" && id.length > 0))].slice(0, LIMIT);
  writePreferences({ ...current, customOrder }, storage);
}

/** Keep persisted ordering bounded to visible recent entries, appending newly seen ids. */
export function orderWikiRecentEntries(
  entries: readonly RecentPageEntry[],
  order: readonly string[],
): readonly RecentPageEntry[] {
  const byId = new Map(entries.slice(0, LIMIT).map(entry => [entry.id, entry]));
  const ids = [...order.filter(id => byId.has(id)), ...entries.slice(0, LIMIT).map(({ id }) => id).filter(id => !order.includes(id))];
  return ids.map(id => byId.get(id)).filter((entry): entry is RecentPageEntry => entry !== undefined);
}
