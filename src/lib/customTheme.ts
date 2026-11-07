// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Applies the daemon's custom theme to every window.
 *
 * The daemon checks theme files and returns a token map that passed a strict
 * grammar; this module only sets those values with `style.setProperty`, never
 * by building CSS text, so a theme cannot add rules or reach the network.
 *
 * The last answer is cached in localStorage (shared by every window of the
 * app) so the first paint already uses the theme, before the daemon answers.
 * The main window asks the daemon for the theme's `revision` every few
 * seconds while it is visible, which is how an edit an AI tool made to a
 * theme file shows up, and tells the other windows over a Tauri event.
 */
import { useSyncExternalStore } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { readPreference, writePreference } from "./preferenceStorage";
import { getActiveTheme, type ActiveTheme, type ThemeBase } from "./tauri";

const CACHE_KEY = "wenlan-custom-theme";
const CHANGED_EVENT = "custom-theme-changed";
export const CUSTOM_THEME_POLL_MS = 4000;

/** What the app applies; images are fetched separately by the main window. */
export type CachedCustomTheme = Pick<ActiveTheme, "id" | "base" | "revision" | "tokens" | "background" | "sticker">;

const listeners = new Set<() => void>();
let applied = new Set<string>();
let current: CachedCustomTheme | null = readCache();

function readCache(): CachedCustomTheme | null {
  const raw = readPreference(CACHE_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as CachedCustomTheme;
    if (typeof parsed?.revision !== "string" || typeof parsed.tokens !== "object") return null;
    return parsed;
  } catch {
    return null;
  }
}

/** Only an active, passing theme is worth applying; anything else is the built-in look. */
function usable(theme: ActiveTheme | CachedCustomTheme | null): CachedCustomTheme | null {
  if (!theme?.id || !theme.base) return null;
  if ("problems" in theme && theme.problems.length > 0) return null;
  const { id, base, revision, tokens, background, sticker } = theme;
  return { id, base, revision, tokens: tokens ?? {}, background, sticker };
}

/** The base (dark or light) the custom theme is built on, or `null` for none. */
export function customThemeBase(): ThemeBase | null {
  return current?.base ?? null;
}

export function getCustomTheme(): CachedCustomTheme | null {
  return current;
}

/**
 * Set the cached theme's tokens on `<html>` and clear the ones a previous
 * theme set. Called by `applyTheme()` right before it stamps `data-theme`, so
 * anything that re-reads colors on that change sees the new values.
 */
export function applyCustomTokens(root: HTMLElement = document.documentElement): void {
  const tokens = current?.tokens ?? {};
  const next = new Set<string>();
  for (const [name, value] of Object.entries(tokens)) {
    if (!name.startsWith("--")) continue;
    root.style.setProperty(name, value);
    next.add(name);
  }
  for (const name of applied) {
    if (!next.has(name)) root.style.removeProperty(name);
  }
  applied = next;
  // The graph and diagrams re-read colors when this changes, even when the
  // base (and so `data-theme`) stays the same.
  root.setAttribute("data-theme-rev", current?.revision ?? "builtin");
}

let applyHook: (() => void) | null = null;

/** `theme.ts` registers its `applyTheme` so a new custom theme re-runs it. */
export function onCustomThemeChange(apply: () => void): void {
  applyHook = apply;
}

function store(next: CachedCustomTheme | null): boolean {
  if ((next?.revision ?? null) === (current?.revision ?? null) && next?.id === current?.id) return false;
  current = next;
  writePreference(CACHE_KEY, next ? JSON.stringify(next) : "");
  applyHook?.();
  for (const fn of listeners) fn();
  return true;
}

/**
 * Ask the daemon for the active theme now and apply it everywhere. Settings
 * calls this right after a save or switch; the poller calls it on a timer.
 */
export async function refreshCustomTheme(answer?: ActiveTheme): Promise<void> {
  let theme: ActiveTheme;
  try {
    theme = answer ?? (await getActiveTheme());
  } catch {
    return; // Daemon not reachable: keep what is on screen.
  }
  const next = usable(theme);
  if (store(next)) emit(CHANGED_EVENT, next).catch(() => {});
}

listen<CachedCustomTheme | null>(CHANGED_EVENT, (event) => {
  store(usable(event.payload));
}).catch(() => {});

/** Main window only: follow edits made outside the app (an AI tool, a text editor). */
export function startCustomThemeSync(): () => void {
  const tick = () => {
    if (document.visibilityState === "visible") void refreshCustomTheme();
  };
  tick();
  const timer = window.setInterval(tick, CUSTOM_THEME_POLL_MS);
  document.addEventListener("visibilitychange", tick);
  return () => {
    window.clearInterval(timer);
    document.removeEventListener("visibilitychange", tick);
  };
}

function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => {
    listeners.delete(fn);
  };
}

export function useCustomTheme(): CachedCustomTheme | null {
  return useSyncExternalStore(subscribe, getCustomTheme, getCustomTheme);
}

/** Test seam: reset module state between tests. */
export function __resetCustomThemeForTests(): void {
  current = readCache();
  applied = new Set();
  applyHook = null;
  listeners.clear();
}
