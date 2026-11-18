// SPDX-License-Identifier: AGPL-3.0-only
//
// Team-member mode: the browser bundle served by a team server at `/app`.
// Every daemon call goes through the server's gate (`/team/v1/app/<path>`)
// with the member's key; a call with no gate row is answered here, without
// a request, the way a desktop-only command is.

import { isWebBuild } from "./runtime";

export const TEAM_KEY_STORAGE = "wenlan.team.key";
export const TEAM_GATE_PREFIX = "/team/v1/app";

/** True when the browser bundle runs from a team server's `/app`. */
export function isTeamMemberMode(): boolean {
  if (!isWebBuild() || typeof window === "undefined") return false;
  const path = window.location.pathname;
  return path === "/app" || path.startsWith("/app/");
}

export function readTeamKey(): string | null {
  try {
    const key = window.localStorage.getItem(TEAM_KEY_STORAGE);
    return key && key.trim() ? key.trim() : null;
  } catch {
    return null;
  }
}

/** Forgets the key in this browser and goes back to the empty `/app`. */
export function forgetTeamKey(): void {
  try {
    window.localStorage.removeItem(TEAM_KEY_STORAGE);
  } catch {
    // Nothing stored, or storage blocked: either way nothing is kept.
  }
  window.location.assign("/app");
}

/**
 * The gate's rows (`crates/wenlan-server/src/team_app_gate.rs`, `CALLS`).
 * `{id}` is one plain segment. A literal row wins over an `{id}` row, as on
 * the server, so `/api/pages/recent` is never read as a page id.
 */
const GATE_ROWS: ReadonlyArray<readonly [string, string]> = [
  ["GET", "/api/health"],
  ["GET", "/api/spaces"],
  ["GET", "/api/pages"],
  ["GET", "/api/pages/recent"],
  ["GET", "/api/pages/recent-changes"],
  ["POST", "/api/pages/search"],
  ["GET", "/api/pages/{id}"],
  ["GET", "/api/pages/{id}/revisions"],
  ["GET", "/api/pages/{id}/sources"],
  ["GET", "/api/memory/graph"],
  ["POST", "/api/memory/{id}/update-page"],
  ["POST", "/api/pages/drafts"],
  ["PUT", "/api/pages/drafts/{id}"],
  ["POST", "/api/pages/drafts/{id}/publish"],
  ["POST", "/api/pages/{id}/archive"],
  ["POST", "/api/pages/{id}/restore"],
  ["PUT", "/api/pages/{id}/basis/{basis_id}"],
  ["DELETE", "/api/pages/{id}/basis/{basis_id}"],
  ["GET", "/api/knowledge/folders"],
  ["POST", "/api/knowledge/folders"],
  ["POST", "/api/pages/{id}/move"],
];

/** Daemon paths the gate routes elsewhere, never as a page id. */
const RESERVED_PAGE_SEGMENTS = new Set([
  "recent", "recent-changes", "search", "drafts", "orphan-links", "basis-pending", "code-basis",
]);

/** Whether the gate has a row for this call (the path may carry a query). */
export function teamCallAllowed(method: string, path: string): boolean {
  const bare = path.split("?")[0];
  if (!/^[A-Za-z0-9/_%-]+$/.test(bare)) return false;
  const given = bare.split("/");
  return GATE_ROWS.some(([rowMethod, rowPath]) => {
    if (rowMethod !== method.toUpperCase()) return false;
    const want = rowPath.split("/");
    if (want.length !== given.length) return false;
    return want.every((segment, index) => {
      const got = given[index];
      if (!segment.startsWith("{")) return segment === got;
      if (!got) return false;
      return !(want[index - 1] === "pages" && index === 3 && RESERVED_PAGE_SEGMENTS.has(got));
    });
  });
}

export class TeamCallUnavailable extends Error {
  constructor(what: string) {
    super(`“${what}” is not available to team members on the web.`);
    this.name = "TeamCallUnavailable";
  }
}

export interface TeamMeSpace {
  space: string;
  access: "read" | "write" | string;
}

export interface TeamMe {
  member_id: string;
  name: string;
  spaces: TeamMeSpace[];
  team?: { name?: string | null } | null;
  /** The people this member shares a Space with, to name who changed a
   *  page: id and name only. Left out by an older server. */
  people?: { id: string; name: string; owner?: boolean }[];
}

let meCache: Promise<TeamMe | null> | null = null;

/** Who the stored key belongs to, and their Spaces; null for no or a dead key. */
export function teamMe(): Promise<TeamMe | null> {
  if (meCache) return meCache;
  const key = readTeamKey();
  if (!key) return Promise.resolve(null);
  meCache = fetch("/team/v1/me", {
    headers: { Authorization: `Bearer ${key}` },
    credentials: "omit",
    cache: "no-store",
  })
    .then(async (response) => (response.ok ? ((await response.json()) as TeamMe) : null))
    .catch(() => null);
  return meCache;
}

/** For tests only. */
export function resetTeamMeForTests(): void {
  meCache = null;
}

/** The member's access in each Space, by Space name. */
export function accessBySpace(me: TeamMe | null): Map<string, string> {
  return new Map((me?.spaces ?? []).map((entry) => [entry.space, entry.access]));
}
