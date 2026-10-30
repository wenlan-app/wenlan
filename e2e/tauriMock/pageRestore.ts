// SPDX-License-Identifier: AGPL-3.0-only
import type { RestorePageErrorCode, RestorePageResponse } from "../../src/lib/tauri";

type RestoreFields = {
  id: string; title: string; version: number; status: string;
  creation_kind?: string | null; kind?: string; space?: string | null;
};

const STATUS: Record<RestorePageErrorCode, string> = {
  page_version_conflict: "409 Conflict",
  page_title_taken: "409 Conflict",
  page_not_archived: "409 Conflict",
  page_title_fixed: "422 Unprocessable Entity",
  page_not_restorable: "422 Unprocessable Entity",
};

/** The same text the Tauri command rejects with when the daemon refuses. */
function refusal(id: string, code: RestorePageErrorCode, error: string, currentVersion?: number): Error {
  const body = JSON.stringify({ code, error, ...(currentVersion === undefined ? {} : { current_version: currentVersion }) });
  return new Error(`HTTP POST /api/pages/${encodeURIComponent(id)}/restore returned ${STATUS[code]}: ${body}`);
}

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
      throw new Error("title must be a non-empty single line of at most 500 characters (422)");
    }
  }
  if (!page) throw new Error("page not found (404)");
  const current = page as T & RestoreFields;
  if (current.kind === "entity" || current.creation_kind === "entity" || current.status === "draft") {
    throw refusal(current.id, "page_not_restorable", "only archived ordinary pages can be restored");
  }
  if (current.version !== expectedVersion) {
    throw refusal(current.id, "page_version_conflict", "page version conflict", current.version);
  }
  title ??= current.title;
  if (current.status === "active" && title === current.title) {
    return { page, response: { id: current.id, title, version: current.version, status: "active", projection_status: "not_configured" } };
  }
  if (current.status !== "archived") {
    throw refusal(current.id, "page_not_archived", "page is no longer archived", current.version);
  }
  if (title !== current.title && ["source", "imported"].includes(current.creation_kind ?? "")) {
    throw refusal(current.id, "page_title_fixed", "source and imported pages keep their original title");
  }
  const space = (value: string | null | undefined) => value ?? "00000000-0000-4000-8000-000000000001";
  const restoredTitle = title;
  if (pages.some((candidate) => {
    const other = candidate as RestoreFields;
    return other.id !== current.id && other.status === "active" && space(other.space) === space(current.space)
      && other.title.trim().toLowerCase() === restoredTitle.trim().toLowerCase();
  })) throw refusal(current.id, "page_title_taken", "page title already exists");
  const version = current.version + 1;
  return {
    page: { ...page, title, status: "active", version, last_modified: new Date().toISOString() },
    response: { id: current.id, title, version, status: "active", projection_status: "not_configured" },
  };
}

/** Archive an active ordinary page in place, as the daemon does: status and version only. */
export function archiveFixturePage<T extends object>(page: T | undefined): T {
  if (!page) throw new Error("page not found (404)");
  const current = page as T & RestoreFields;
  if (current.status !== "active" || current.creation_kind === "entity") {
    throw new Error("only active ordinary pages can be archived (409)");
  }
  return { ...page, status: "archived", version: current.version + 1, last_modified: new Date().toISOString() };
}
