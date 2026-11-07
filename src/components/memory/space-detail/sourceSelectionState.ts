// SPDX-License-Identifier: AGPL-3.0-only
const MAX_SELECTED = 100;
const selectionKey = (spaceId: string) => `wenlan:source-selection:${spaceId}`;

export function readSourceSelection(spaceId: string): string[] {
  try {
    const raw = sessionStorage.getItem(selectionKey(spaceId));
    const parsed: unknown = raw ? JSON.parse(raw) : [];
    return Array.isArray(parsed) ? parsed.filter((value): value is string => typeof value === "string").slice(0, MAX_SELECTED) : [];
  } catch { return []; }
}

export function writeSourceSelection(spaceId: string, ids: readonly string[]): void {
  try { sessionStorage.setItem(selectionKey(spaceId), JSON.stringify([...new Set(ids)].slice(0, MAX_SELECTED))); } catch { /* Optional session continuity. */ }
}

export function readOrganizeDraft(spaceId: string): string | null {
  try { return sessionStorage.getItem(`wenlan:source-organize:draft:${spaceId}`); } catch { return null; }
}

export function writeOrganizeDraft(spaceId: string, text: string): void {
  try { sessionStorage.setItem(`wenlan:source-organize:draft:${spaceId}`, text.slice(0, 8000)); } catch { /* Optional session continuity. */ }
}
