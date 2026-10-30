// SPDX-License-Identifier: AGPL-3.0-only
import type { RestorePageResponse } from "../../src/lib/tauri";

type RestoreFields = {
  id: string; title: string; version: number; status: string;
  creation_kind?: string | null; kind?: string; space?: string | null;
};

/** In-memory restore only: no projection file, embedding, or daemon work. */
export function restoreFixturePage<T extends object>(
  page: T | undefined,
  pages: readonly object[],
  expectedVersion: unknown,
  requestedTitle?: unknown,
): { page: T; response: RestorePageResponse } {
  if (typeof expectedVersion !== "number" || !Number.isSafeInteger(expectedVersion) || expectedVersion < 1) {
    throw new Error("expected_version must be at least 1 (400)");
  }
  let title: string | undefined;
  if (requestedTitle != null) {
    if (typeof requestedTitle !== "string") throw new Error("invalid page title (400)");
    title = requestedTitle.trim();
    if (!title || [...title].length > 500 || /[\u0000-\u001f\u007f-\u009f]/.test(title)) {
      throw new Error("title must be a non-empty single line of at most 500 characters (400)");
    }
  }
  if (!page) throw new Error("page not found (404)");
  const current = page as T & RestoreFields;
  if (current.kind === "entity" || current.creation_kind === "entity" || current.status === "draft") {
    throw new Error("only archived ordinary pages can be restored (400)");
  }
  if (current.version !== expectedVersion) throw new Error("page version conflict (409)");
  title ??= current.title;
  if (current.status === "active" && title === current.title) {
    return { page, response: { id: current.id, title, version: current.version, status: "active", projection_status: "not_configured" } };
  }
  if (current.status !== "archived") throw new Error("page is no longer archived (409)");
  if (title !== current.title && ["source", "imported"].includes(current.creation_kind ?? "")) {
    throw new Error("source and imported notes must retain their original title (400)");
  }
  const space = (value: string | null | undefined) => value ?? "00000000-0000-4000-8000-000000000001";
  if (pages.some((candidate) => {
    const other = candidate as RestoreFields;
    return other.id !== current.id && other.status === "active" && space(other.space) === space(current.space)
      && other.title.trim().toLowerCase() === title.trim().toLowerCase();
  })) throw new Error("page title already exists (409)");
  const version = current.version + 1;
  return {
    page: { ...page, title, status: "active", version, last_modified: new Date().toISOString() },
    response: { id: current.id, title, version, status: "active", projection_status: "not_configured" },
  };
}
