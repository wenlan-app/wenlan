// SPDX-License-Identifier: AGPL-3.0-only
import { readPreference, writePreference } from "../../../lib/preferenceStorage";
import type { GlobalNavigation } from "./viewState";

// The stream of memories is still a view (a memory opens there) but has no
// place in the navigation.
export type NavigationDestination = Exclude<GlobalNavigation, "memories">;
export type SidebarMode = "icons" | "labels";
export type SidebarPreferences = { readonly visible: boolean; readonly mode: SidebarMode; readonly width?: number };

export const NAVIGATION_PREFERENCE_KEY = "wenlan-navigation-v1";
export const SIDEBAR_PREFERENCE_KEY = "wenlan-sidebar-collapsed";
export const LEGACY_SIDEBAR_PREFERENCE_KEY = "origin-sidebar-collapsed";
// Home is where the app opens (what you're working on); the Wiki holds every
// page; Spaces split the work by project. Sources, the graph and topics wait
// under More until you pin them. Memories have no page of their own: you meet
// them through search and the things they belong to.
export const NAVIGATION_DESTINATION_ORDER: readonly NavigationDestination[] = [
  "home", "pages", "spaces", "sources", "graph", "entities",
];
export const REQUIRED_NAVIGATION_DESTINATIONS: readonly NavigationDestination[] = ["home", "pages", "spaces"];
export const DEFAULT_VISIBLE_NAVIGATION: readonly NavigationDestination[] = REQUIRED_NAVIGATION_DESTINATIONS;
export const DEFAULT_SIDEBAR_PREFERENCES: SidebarPreferences = { visible: true, mode: "labels" };
export const DEFAULT_SIDEBAR_WIDTH = 240;
export const MIN_SIDEBAR_WIDTH = 200;
export const MAX_SIDEBAR_WIDTH = 360;
export const ICONS_SIDEBAR_WIDTH = 64;

// Version 2 moved sources and the graph under More. A version 1 payload
// pinned them only because they were required then, so its pins are dropped
// and its sidebar width and mode are kept.
const NAVIGATION_PREFERENCE_VERSION = 2;
type NavigationPreferencePayload = { version: 1 | 2; visible: unknown[]; sidebar?: unknown };

function readPayload(): NavigationPreferencePayload | null {
  const stored = readPreference(NAVIGATION_PREFERENCE_KEY);
  if (stored === null) return null;
  try {
    const payload: unknown = JSON.parse(stored);
    if (typeof payload !== "object" || payload === null || !("version" in payload) || (payload.version !== 1 && payload.version !== NAVIGATION_PREFERENCE_VERSION) || !("visible" in payload) || !Array.isArray(payload.visible)) return null;
    const read = payload as NavigationPreferencePayload;
    return read.version === 1 ? { ...read, visible: [...DEFAULT_VISIBLE_NAVIGATION] } : read;
  } catch {
    return null;
  }
}

function legacySidebarPreferences(): SidebarPreferences {
  const stored = readPreference(SIDEBAR_PREFERENCE_KEY, LEGACY_SIDEBAR_PREFERENCE_KEY);
  if (stored === "true") return { visible: false, mode: "labels" };
  if (stored === "false") return { visible: true, mode: "labels" };
  return DEFAULT_SIDEBAR_PREFERENCES;
}

function parseSidebarPreferences(value: unknown): SidebarPreferences | null {
  if (typeof value !== "object" || value === null || !("visible" in value) || !("mode" in value)) return null;
  const preference = value as { visible: unknown; mode: unknown; width?: unknown };
  if (typeof preference.visible !== "boolean" || (preference.mode !== "icons" && preference.mode !== "labels")) return null;
  const width = sanitizeSidebarWidth(preference.width);
  return { visible: preference.visible, mode: preference.mode, ...(width === undefined ? {} : { width }) };
}

export function sanitizeSidebarWidth(value: unknown): number | undefined {
  if (typeof value !== "number" || !Number.isFinite(value)) return undefined;
  return Math.round(Math.min(MAX_SIDEBAR_WIDTH, Math.max(MIN_SIDEBAR_WIDTH, value)));
}

export function expandedSidebarWidth(preferences: SidebarPreferences): number {
  return sanitizeSidebarWidth(preferences.width) ?? DEFAULT_SIDEBAR_WIDTH;
}

export function sidebarLayoutWidth(preferences: SidebarPreferences): number {
  if (!preferences.visible) return 0;
  return preferences.mode === "icons" ? ICONS_SIDEBAR_WIDTH : expandedSidebarWidth(preferences);
}

export function sanitizeVisibleNavigation(visible: readonly unknown[]): NavigationDestination[] {
  return NAVIGATION_DESTINATION_ORDER.filter((destination) =>
    REQUIRED_NAVIGATION_DESTINATIONS.includes(destination) || visible.includes(destination),
  );
}

export function readNavigationPreferences(): NavigationDestination[] {
  const payload = readPayload();
  return sanitizeVisibleNavigation(payload?.visible ?? DEFAULT_VISIBLE_NAVIGATION);
}

export function readSidebarPreferences(): SidebarPreferences {
  const payload = readPayload();
  if (payload && "sidebar" in payload) return parseSidebarPreferences(payload.sidebar) ?? DEFAULT_SIDEBAR_PREFERENCES;
  return legacySidebarPreferences();
}

export function writeNavigationPreferences(visible: readonly GlobalNavigation[]): void {
  writePreference(NAVIGATION_PREFERENCE_KEY, JSON.stringify({
    version: NAVIGATION_PREFERENCE_VERSION,
    visible: sanitizeVisibleNavigation(visible),
    sidebar: readSidebarPreferences(),
  }));
}

export function writeSidebarPreferences(sidebar: SidebarPreferences): void {
  writePreference(NAVIGATION_PREFERENCE_KEY, JSON.stringify({
    version: NAVIGATION_PREFERENCE_VERSION,
    visible: readNavigationPreferences(),
    sidebar: parseSidebarPreferences(sidebar) ?? DEFAULT_SIDEBAR_PREFERENCES,
  }));
}
