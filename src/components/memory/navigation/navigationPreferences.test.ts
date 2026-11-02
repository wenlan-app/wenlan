// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SIDEBAR_PREFERENCES, DEFAULT_SIDEBAR_WIDTH, DEFAULT_VISIBLE_NAVIGATION, REQUIRED_NAVIGATION_DESTINATIONS, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH, NAVIGATION_PREFERENCE_KEY, expandedSidebarWidth, readNavigationPreferences, readSidebarPreferences, sanitizeSidebarWidth, sanitizeVisibleNavigation, writeNavigationPreferences, writeSidebarPreferences } from "./navigationPreferences";

beforeEach(() => {
  localStorage.clear();
  vi.restoreAllMocks();
});

describe("navigation preferences", () => {
  it("defaults when no preference is saved", () => {
    expect(readNavigationPreferences()).toEqual(DEFAULT_VISIBLE_NAVIGATION);
    expect(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)).toBeNull();
  });

  it.each([
    "{", "null", "[]", '{}', '{"version":2,"visible":["home"]}',
    '{"version":1,"visible":"home"}',
  ])("defaults for malformed or unsupported data %s", (payload) => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, payload);
    expect(readNavigationPreferences()).toEqual(DEFAULT_VISIBLE_NAVIGATION);
  });

  it("restores required destinations when no optional pins are saved", () => {
    writeNavigationPreferences([]);
    expect(readNavigationPreferences()).toEqual(REQUIRED_NAVIGATION_DESTINATIONS);
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!).visible).toEqual(REQUIRED_NAVIGATION_DESTINATIONS);
  });

  it("keeps Home first, once, while preserving other destinations", () => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, JSON.stringify({ version: 1, visible: ["home", "sources", "home", "memories"] }));
    expect(readNavigationPreferences()).toEqual(["home", "pages", "spaces", "graph", "sources", "memories"]);
    expect(sanitizeVisibleNavigation(["home", "pages", "home"])).toEqual(REQUIRED_NAVIGATION_DESTINATIONS);
  });

  it.each([["home"], ["unknown", null, 3]])("restores required destinations from obsolete saved pins %j", (...visible) => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, JSON.stringify({ version: 1, visible }));
    expect(readNavigationPreferences()).toEqual(REQUIRED_NAVIGATION_DESTINATIONS);
  });

  it("keeps known unique keys in canonical order", () => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, JSON.stringify({ version: 1, visible: ["home", "unknown", "spaces", "home", "pages"] }));
    expect(readNavigationPreferences()).toEqual(["home", "pages", "spaces", "graph", "sources"]);
    expect(sanitizeVisibleNavigation(["entities", "graph", "graph", null])).toEqual(["home", "pages", "spaces", "graph", "sources", "entities"]);
    writeNavigationPreferences(["entities", "sources", "sources", "pages"]);
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!)).toEqual({
      version: 1,
      visible: ["home", "pages", "spaces", "graph", "sources", "entities"],
      sidebar: DEFAULT_SIDEBAR_PREFERENCES,
    });
  });

  it.each([
    ["true", { visible: false, mode: "labels" }],
    ["false", { visible: true, mode: "labels" }],
  ] as const)("upgrades legacy sidebar preference %s to %s", (legacyValue, expected) => {
    localStorage.setItem("wenlan-sidebar-collapsed", legacyValue);
    expect(readSidebarPreferences()).toEqual(expected);
  });

  it("defaults invalid sidebar settings and persists visibility plus mode with navigation pins", () => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, JSON.stringify({ version: 1, visible: ["pages"], sidebar: { visible: "yes", mode: "compact" } }));
    expect(readSidebarPreferences()).toEqual(DEFAULT_SIDEBAR_PREFERENCES);

    writeSidebarPreferences({ visible: false, mode: "icons" });
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!)).toEqual({
      version: 1,
      visible: ["home", "pages", "spaces", "graph", "sources"],
      sidebar: { visible: false, mode: "icons" },
    });
    writeNavigationPreferences(["spaces"]);
    expect(readSidebarPreferences()).toEqual({ visible: false, mode: "icons" });
  });

  it("keeps width optional in old payloads and uses the default expanded width", () => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, JSON.stringify({ version: 1, visible: ["pages"], sidebar: { visible: true, mode: "labels" } }));
    const sidebar = readSidebarPreferences();
    expect(sidebar).toEqual({ visible: true, mode: "labels" });
    expect(expandedSidebarWidth(sidebar)).toBe(DEFAULT_SIDEBAR_WIDTH);
    writeSidebarPreferences(sidebar);
    expect(JSON.parse(localStorage.getItem(NAVIGATION_PREFERENCE_KEY)!).sidebar).toEqual({ visible: true, mode: "labels" });
  });

  it.each([
    [120, MIN_SIDEBAR_WIDTH],
    [MIN_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH],
    [287.4, 287],
    [MAX_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH],
    [900, MAX_SIDEBAR_WIDTH],
  ])("clamps a finite sidebar width %s to %s", (input, expected) => {
    expect(sanitizeSidebarWidth(input)).toBe(expected);
  });

  it("ignores invalid optional widths and reads saved finite widths within bounds", () => {
    localStorage.setItem(NAVIGATION_PREFERENCE_KEY, JSON.stringify({ version: 1, visible: ["pages"], sidebar: { visible: true, mode: "labels", width: "300" } }));
    expect(readSidebarPreferences()).toEqual({ visible: true, mode: "labels" });

    writeSidebarPreferences({ visible: true, mode: "labels", width: 315 });
    expect(readSidebarPreferences()).toEqual({ visible: true, mode: "labels", width: 315 });
    expect(expandedSidebarWidth(readSidebarPreferences())).toBe(315);

    writeNavigationPreferences(["sources"]);
    expect(readSidebarPreferences()).toEqual({ visible: true, mode: "labels", width: 315 });
  });

  it("falls back safely for malformed and unsupported sidebar preference payloads", () => {
    for (const payload of ["{", JSON.stringify({ version: 2, visible: ["pages"], sidebar: { visible: false, mode: "icons" } })]) {
      localStorage.setItem(NAVIGATION_PREFERENCE_KEY, payload);
      expect(readSidebarPreferences()).toEqual(DEFAULT_SIDEBAR_PREFERENCES);
    }
  });

  it("continues when storage cannot be accessed or written", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("disabled"); });
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("disabled"); });
    expect(readNavigationPreferences()).toEqual(DEFAULT_VISIBLE_NAVIGATION);
    expect(() => writeNavigationPreferences(["sources"])).not.toThrow();
  });
});
