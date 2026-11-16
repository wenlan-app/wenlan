// SPDX-License-Identifier: AGPL-3.0-only
import { useSyncExternalStore } from "react";
import { readPreference, writePreference } from "../../../lib/preferenceStorage";

/** A page the person put on the left side (UX-FIND-6). Kept on this
 *  computer, like a page's look; the title is what the page was called when
 *  it was last seen, so the row can show before the page loads. */
export type PinnedPage = { readonly id: string; readonly title: string };

export const PINNED_PAGES_PREFERENCE_KEY = "wenlan-pinned-pages-v1";

const listeners = new Set<() => void>();
let cachedRaw: string | null | undefined;
let cached: readonly PinnedPage[] = [];

function parse(raw: string | null): readonly PinnedPage[] {
  if (raw === null) return [];
  try {
    const payload: unknown = JSON.parse(raw);
    if (!Array.isArray(payload)) return [];
    const seen = new Set<string>();
    return payload.flatMap((entry: unknown) => {
      if (typeof entry !== "object" || entry === null) return [];
      const { id, title } = entry as { id?: unknown; title?: unknown };
      if (typeof id !== "string" || !id || typeof title !== "string" || seen.has(id)) return [];
      seen.add(id);
      return [{ id, title }];
    });
  } catch {
    return [];
  }
}

export function readPinnedPages(): readonly PinnedPage[] {
  const raw = readPreference(PINNED_PAGES_PREFERENCE_KEY);
  if (raw !== cachedRaw) {
    cachedRaw = raw;
    cached = parse(raw);
  }
  return cached;
}

function write(pages: readonly PinnedPage[]): void {
  writePreference(PINNED_PAGES_PREFERENCE_KEY, JSON.stringify(pages));
  // Storage can be unavailable; the list still holds for this window.
  cachedRaw = readPreference(PINNED_PAGES_PREFERENCE_KEY);
  cached = pages;
  for (const listener of listeners) listener();
}

export function isPagePinned(id: string): boolean {
  return readPinnedPages().some((page) => page.id === id);
}

export function pinPage(page: PinnedPage): void {
  if (isPagePinned(page.id)) return;
  write([...readPinnedPages(), page]);
}

export function unpinPage(id: string): void {
  if (!isPagePinned(id)) return;
  write(readPinnedPages().filter((page) => page.id !== id));
}

/** Keeps a pinned row's name in step with its page. */
export function renamePinnedPage(id: string, title: string): void {
  const pages = readPinnedPages();
  if (!pages.some((page) => page.id === id && page.title !== title)) return;
  write(pages.map((page) => (page.id === id ? { id, title } : page)));
}

/** Moves `id` to sit just before `beforeId`, or to the end when null. */
export function movePinnedPage(id: string, beforeId: string | null): void {
  const pages = readPinnedPages();
  const moved = pages.find((page) => page.id === id);
  if (!moved || id === beforeId) return;
  const rest = pages.filter((page) => page.id !== id);
  const at = beforeId === null ? -1 : rest.findIndex((page) => page.id === beforeId);
  const next = at < 0 ? [...rest, moved] : [...rest.slice(0, at), moved, ...rest.slice(at)];
  if (next.every((page, index) => page.id === pages[index]?.id)) return;
  write(next);
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

export function usePinnedPages(): readonly PinnedPage[] {
  return useSyncExternalStore(subscribe, readPinnedPages, readPinnedPages);
}
