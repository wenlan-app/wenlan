// SPDX-License-Identifier: AGPL-3.0-only
import type { Page } from "../../../lib/tauri";

export type PageProjectionIssue = { readonly expectedVersion: number };

export type WikiInventoryScope = "all" | `folder:${string}`;
export function folderScope(path: string): WikiInventoryScope { return `folder:${path}`; }
export function inventoryFolderPath(scope: WikiInventoryScope): string | null { return scope === "all" ? null : scope.slice(7); }

/** Only daemon-verified, safe knowledge-root relative paths are physical notes. */
export function inventoryPageFilename(page: Page): string | null {
  const path = page.storage_path;
  if (!path || path.trim() !== path || path.startsWith("/") || /[\\:\u0000-\u001f\u007f]/u.test(path) || !/\.md$/iu.test(path)) return null;
  if (path.split("/").some(part => !part || part.startsWith(".") || part.trim() !== part)) return null;
  return path;
}
export function pageFolderPath(page: Page): string | null {
  const path = inventoryPageFilename(page);
  if (path) return path.includes("/") ? path.slice(0, path.lastIndexOf("/")) : "";
  return page.status === "draft" && typeof page.folder_path === "string" ? page.folder_path : null;
}
export function pageInventoryScope(page: Page): WikiInventoryScope | null {
  // Only an entity's own shadow page stays out of the inventory. A note
  // distilled about an entity also carries `entity_id`, and it is an ordinary
  // note with a file in the folder.
  if (page.creation_kind === "entity") return null;
  const path = pageFolderPath(page);
  return path === null ? "all" : folderScope(path);
}
export function pageMatchesInventoryScope(page: Page, scope: WikiInventoryScope): boolean {
  return pageInventoryScope(page) !== null && (scope === "all" || pageFolderPath(page) === inventoryFolderPath(scope));
}

function modifiedAt(page: Page): number {
  const value = Date.parse(page.last_modified || page.last_compiled || page.created_at);
  return Number.isFinite(value) ? value : 0;
}

export function collectPageInventory(active: readonly Page[], drafts: readonly Page[]): Page[] {
  const byId = new Map<string, Page>();
  // Active wins during publication while the independent draft query catches up.
  for (const page of [...drafts, ...active]) {
    if (pageInventoryScope(page) !== null) byId.set(page.id, page);
  }
  return [...byId.values()].sort(
    (left, right) => modifiedAt(right) - modifiedAt(left) || left.title.localeCompare(right.title),
  );
}

export function filterPageInventory(pages: readonly Page[], filter: string, locale: string): Page[] {
  const normalized = filter.trim().toLocaleLowerCase(locale);
  if (!normalized) return [...pages];
  return pages.filter((page) =>
    page.title.toLocaleLowerCase(locale).includes(normalized)
    || (inventoryPageFilename(page) ?? "").toLocaleLowerCase(locale).includes(normalized),
  );
}
