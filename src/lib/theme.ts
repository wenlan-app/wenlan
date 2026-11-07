// SPDX-License-Identifier: AGPL-3.0-only
import { useSyncExternalStore } from "react";
import { emit, listen } from "@tauri-apps/api/event";
import { readPreference, writePreference } from "./preferenceStorage";
import { applyCustomTokens, customThemeBase, onCustomThemeChange } from "./customTheme";

export type Theme = "system" | "light" | "dark";

const STORAGE_KEY = "wenlan-theme";
const LEGACY_STORAGE_KEY = "origin-theme";
const listeners = new Set<() => void>();
let current: Theme = (readPreference(STORAGE_KEY, LEGACY_STORAGE_KEY) as Theme) ?? "system";

function notify() {
  for (const fn of listeners) fn();
}

function resolveTheme(theme: Theme): "light" | "dark" {
  if (theme !== "system") return theme;
  return window.matchMedia("(prefers-color-scheme: dark)").matches
    ? "dark"
    : "light";
}

/**
 * Apply the resolved theme to <html data-theme="...">. A custom theme picks
 * its own base (dark or light) and sets its tokens first, so whatever
 * re-reads colors when `data-theme` changes sees the new values.
 */
export function applyTheme(theme?: Theme) {
  const t = theme ?? current;
  const resolved = customThemeBase() ?? resolveTheme(t);
  applyCustomTokens();
  document.documentElement.setAttribute("data-theme", resolved);
}

onCustomThemeChange(() => applyTheme());

export function getTheme(): Theme {
  return current;
}

export function setTheme(theme: Theme) {
  current = theme;
  writePreference(STORAGE_KEY, theme);
  applyTheme(theme);
  notify();
  // Broadcast to all Tauri windows so quick capture, toast, etc. stay in sync
  emit("theme-changed", theme).catch(() => {});
}

// Listen for theme changes broadcast from other windows
listen<string>("theme-changed", (event) => {
  const incoming = event.payload as Theme;
  if (incoming && incoming !== current) {
    current = incoming;
    writePreference(STORAGE_KEY, incoming);
    applyTheme(incoming);
    notify();
  }
}).catch(() => {});

// Listen for OS color scheme changes — re-apply when in "system" mode
const mq = window.matchMedia("(prefers-color-scheme: dark)");
mq.addEventListener("change", () => {
  if (current === "system") applyTheme();
});

// React hook
function subscribe(fn: () => void) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function useTheme(): [Theme, (t: Theme) => void] {
  const theme = useSyncExternalStore(subscribe, getTheme, getTheme);
  return [theme, setTheme];
}
