// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it } from "vitest";
import {
  orderWikiRecentEntries,
  readWikiCustomOrder,
  readWikiInventoryMode,
  writeWikiCustomOrder,
  writeWikiInventoryMode,
  type WikiNotesStorage,
} from "./wikiNotesPreferences";

function memoryStorage(initial: string | null = null) {
  let value = initial;
  const storage: WikiNotesStorage = {
    getItem: () => value,
    setItem: (_key, next) => { value = next; },
  };
  return { storage, value: () => value };
}

const entries = [
  { id: "a", title: "A", visitedAt: 3 },
  { id: "b", title: "B", visitedAt: 2 },
  { id: "c", title: "C", visitedAt: 1 },
];

describe("wiki notes preferences", () => {
  it("persists the selected inventory mode and custom order", () => {
    const cache = memoryStorage();
    expect(readWikiInventoryMode(cache.storage)).toBe("recent");
    writeWikiInventoryMode("custom", cache.storage);
    writeWikiCustomOrder(["b", "a"], cache.storage);
    expect(readWikiInventoryMode(cache.storage)).toBe("custom");
    expect(readWikiCustomOrder(cache.storage)).toEqual(["b", "a"]);
    expect(cache.value()).toContain('"version":1');
  });

  it("defaults malformed or blocked storage safely and still returns current entries", () => {
    const corrupt = memoryStorage("not-json");
    const blocked: WikiNotesStorage = { getItem: () => { throw new Error("blocked"); }, setItem: () => { throw new Error("blocked"); } };
    expect(readWikiInventoryMode(corrupt.storage)).toBe("recent");
    expect(readWikiCustomOrder(corrupt.storage)).toEqual([]);
    expect(() => writeWikiInventoryMode("custom", blocked)).not.toThrow();
    expect(() => writeWikiCustomOrder(["c"], blocked)).not.toThrow();
    expect(orderWikiRecentEntries(entries, ["c", "missing"])).toEqual([entries[2], entries[0], entries[1]]);
  });

  it("keeps existing custom positions and appends new visible entries", () => {
    expect(orderWikiRecentEntries(entries, ["c", "a"])).toEqual([entries[2], entries[0], entries[1]]);
    const refreshed = [{ ...entries[0], title: "Renamed" }, entries[1], entries[2]];
    expect(orderWikiRecentEntries(refreshed, ["c", "a"])).toEqual([entries[2], refreshed[0], entries[1]]);
  });
});

describe("Archived pages browse mode", () => {
  it("is never remembered, so the app never reopens in it", () => {
    const store = new Map<string, string>();
    const storage = { getItem: (key: string) => store.get(key) ?? null, setItem: (key: string, value: string) => { store.set(key, value); } };
    writeWikiInventoryMode("folders", storage);
    writeWikiInventoryMode("archived", storage);
    expect(readWikiInventoryMode(storage)).toBe("folders");
    store.set("wenlan:wiki-inventory:v1", JSON.stringify({ version: 1, mode: "archived", customOrder: [] }));
    expect(readWikiInventoryMode(storage)).toBe("recent");
  });
});
