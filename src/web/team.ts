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

export interface TeamLinkedAccount {
  /** "google" or "github". */
  provider: string;
  /** The email the provider last showed; never an account id. */
  email?: string | null;
}

export interface TeamMe {
  member_id: string;
  name: string;
  spaces: TeamMeSpace[];
  team?: { name?: string | null } | null;
  /** The people this member shares a Space with, to name who changed a
   *  page: id and name only. Left out by an older server. */
  people?: { id: string; name: string; owner?: boolean }[];
  /** How this request signed in: a browser session or the member's key.
   *  Left out by an older server. */
  signed_in_with?: "browser" | "key";
  /** Accounts linked to the member. Left out when there are none. */
  accounts?: TeamLinkedAccount[];
}

let meCache: Promise<TeamMe | null> | null = null;
let credential: "browser" | "key" | null = null;

async function readMe(init: RequestInit): Promise<TeamMe | null> {
  try {
    const response = await fetch("/team/v1/me", { ...init, cache: "no-store" });
    return response.ok ? ((await response.json()) as TeamMe) : null;
  } catch {
    return null;
  }
}

/** Takes the key out of this browser's storage, without leaving the page. */
function dropTeamKey(): void {
  try {
    window.localStorage.removeItem(TEAM_KEY_STORAGE);
  } catch {
    // Storage blocked: nothing was kept to drop.
  }
}

async function resolveMe(): Promise<TeamMe | null> {
  // The browser session comes first: a 401 only means it is not signed in.
  const byCookie = await readMe({ credentials: "same-origin" });
  if (byCookie) {
    credential = "browser";
    // The old key goes only now, when the session is known to work.
    if (readTeamKey()) dropTeamKey();
    return { ...byCookie, signed_in_with: "browser" };
  }
  const key = readTeamKey();
  if (!key) return null;
  const byKey = await readMe({ headers: { Authorization: `Bearer ${key}` }, credentials: "omit" });
  if (byKey) credential = "key";
  return byKey ? { ...byKey, signed_in_with: "key" } : null;
}

/** Who this browser is: by its session cookie, else by the stored key. Null
 *  when it is signed in neither way (a 401), or the key is dead. */
export function teamMe(): Promise<TeamMe | null> {
  if (!meCache) meCache = resolveMe();
  return meCache;
}

/** How the calls to the gate sign in, once `teamMe` knows. */
export function teamCredential(): "browser" | "key" | null {
  return credential;
}

/** Forgets who this browser is: after signing out, or a session that ended. */
export function resetTeamMe(): void {
  meCache = null;
  credential = null;
}

/** For tests only. */
export function resetTeamMeForTests(): void {
  resetTeamMe();
  providersCache = null;
}

async function endSessions(path: string): Promise<void> {
  try {
    await fetch(path, { method: "POST", credentials: "same-origin", cache: "no-store" });
  } catch {
    // Offline: the cookie stays, and the reload below shows the truth.
  }
  resetTeamMe();
  window.location.assign("/app");
}

/** Ends this browser's session, then reloads `/app`. */
export function signOutBrowser(): Promise<void> {
  return endSessions("/auth/logout");
}

/** Ends every browser session of this member (AI tool keys keep working),
 *  then reloads `/app`. */
export function signOutEverywhere(): Promise<void> {
  return endSessions("/auth/logout-all");
}

export interface TeamProviders {
  providers: string[];
  team: string | null;
}

let providersCache: Promise<TeamProviders> | null = null;

/** Which providers the server signs people in with, and the team's name;
 *  none when sign-in is off or the server is older than this. */
export function teamProviders(): Promise<TeamProviders> {
  if (providersCache) return providersCache;
  providersCache = (async () => {
    try {
      const response = await fetch("/auth/providers", { credentials: "omit", cache: "no-store" });
      if (!response.ok) return { providers: [], team: null };
      const body = (await response.json()) as { providers?: unknown; team?: unknown };
      const providers = Array.isArray(body.providers)
        ? body.providers.filter((name): name is string => name === "google" || name === "github")
        : [];
      return { providers, team: typeof body.team === "string" && body.team ? body.team : null };
    } catch {
      return { providers: [], team: null };
    }
  })();
  return providersCache;
}

/**
 * Starts adding a Google or GitHub account to a member who joined with a
 * key. The server answers with the provider's address; the browser goes
 * there, the provider sends it back to the server's callback, and that makes
 * a session cookie and lands on `/app`. The next load of `/app` finds the
 * cookie working and only then drops the key (see `resolveMe`).
 * Resolves false when the server would not start it.
 */
export async function linkProvider(provider: string): Promise<boolean> {
  const key = readTeamKey();
  if (!key) return false;
  try {
    const response = await fetch(`/auth/link/${encodeURIComponent(provider)}`, {
      method: "POST",
      headers: { Authorization: `Bearer ${key}` },
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!response.ok) return false;
    const answer = (await response.json()) as { url?: unknown };
    if (typeof answer.url !== "string" || !answer.url) return false;
    window.location.assign(answer.url);
    return true;
  } catch {
    return false;
  }
}

/** The member's access in each Space, by Space name. */
export function accessBySpace(me: TeamMe | null): Map<string, string> {
  return new Map((me?.spaces ?? []).map((entry) => [entry.space, entry.access]));
}

/** One of the member's AI tools: its name and when it was added. */
export interface TeamTool {
  id: string;
  name: string;
  created_at: string;
}

/** How a call to the member's own account signs in: the browser session,
 *  or the key kept in this browser. */
function accountInit(init: RequestInit = {}): RequestInit {
  const key = credential === "key" ? readTeamKey() : null;
  return {
    ...init,
    headers: { ...(init.body ? { "Content-Type": "application/json" } : {}), ...(key ? { Authorization: `Bearer ${key}` } : {}) },
    credentials: key ? "omit" : "same-origin",
    cache: "no-store",
  };
}

/** The member's AI tools; null when the server would not say. */
export async function teamTools(): Promise<TeamTool[] | null> {
  try {
    const response = await fetch("/team/v1/account/keys", accountInit());
    return response.ok ? ((await response.json()) as TeamTool[]) : null;
  } catch {
    return null;
  }
}

/** Removes one of them; resolves to the tools left, or null on failure. */
export async function removeTeamTool(id: string): Promise<TeamTool[] | null> {
  try {
    const response = await fetch(`/team/v1/account/keys/${encodeURIComponent(id)}`, accountInit({ method: "DELETE" }));
    return response.ok ? ((await response.json()) as TeamTool[]) : null;
  } catch {
    return null;
  }
}

/** Unlinks the member's Google or GitHub account; resolves to the accounts
 *  left. Refused with `last_sign_in` when it is their only way in. */
export function unlinkAccount(provider: string): Promise<Answer<TeamLinkedAccount[]>> {
  return answer(`/team/v1/account/accounts/${encodeURIComponent(provider)}`, undefined, "DELETE");
}

/** What a call that may need a fresh sign-in answers. */
export type Answer<T> = { ok: true; value: T } | { ok: false; error: string };

async function answer<T>(path: string, body?: unknown, method = "POST"): Promise<Answer<T>> {
  try {
    const response = await fetch(path, accountInit({ method, body: body === undefined ? undefined : JSON.stringify(body) }));
    if (response.ok) {
      const text = await response.text();
      return { ok: true, value: (text ? JSON.parse(text) : null) as T };
    }
    const said = (await response.json().catch(() => ({}))) as { error?: unknown };
    return { ok: false, error: typeof said.error === "string" ? said.error : `http_${response.status}` };
  } catch {
    return { ok: false, error: "offline" };
  }
}

export interface DeviceRequest {
  tool: string;
  spaces: { space: string; access: string }[];
}

/** Which AI tool waits behind a short code. */
export function deviceLookup(code: string): Promise<Answer<DeviceRequest>> {
  return answer("/auth/device/lookup", { user_code: code });
}

/** Allows (or refuses) the AI tool behind a short code. Allowing answers
 *  `reauth_required` when the browser has not signed in again just now. */
export function deviceDecide(code: string, allow: boolean): Promise<Answer<null>> {
  return answer(allow ? "/auth/device/approve" : "/auth/device/deny", { user_code: code });
}

/** A one-time link for the desktop app, with a new member key. Answers
 *  `reauth_required` when the browser has not signed in again just now. */
export function desktopLink(): Promise<Answer<{ link: string }>> {
  return answer("/team/v1/account/desktop-link");
}

/** What to pick up again after the provider sends the browser back. */
export type After = { kind: "device"; code?: string } | { kind: "desktop" };

export const AFTER_STORAGE = "wenlan.team.after";

export function rememberAfter(after: After): void {
  try {
    window.sessionStorage.setItem(AFTER_STORAGE, JSON.stringify(after));
  } catch {
    // Storage blocked: the person starts over by hand.
  }
}

/** What was left to do before signing in again, read once. */
export function takeAfter(): After | null {
  try {
    const raw = window.sessionStorage.getItem(AFTER_STORAGE);
    window.sessionStorage.removeItem(AFTER_STORAGE);
    const after = raw ? (JSON.parse(raw) as After) : null;
    return after && (after.kind === "device" || after.kind === "desktop") ? after : null;
  } catch {
    return null;
  }
}

/** Looks without taking. */
export function peekAfter(): After | null {
  try {
    const raw = window.sessionStorage.getItem(AFTER_STORAGE);
    return raw ? (JSON.parse(raw) as After) : null;
  } catch {
    return null;
  }
}

/**
 * Signs in again with an account already linked, then comes back to `/app`
 * and picks up `after`. Resolves false when the server would not start it.
 */
export async function reauthThen(provider: string, after: After): Promise<boolean> {
  rememberAfter(after);
  try {
    const response = await fetch(`/auth/reauth/${encodeURIComponent(provider)}`, {
      method: "POST",
      credentials: "same-origin",
      cache: "no-store",
    });
    if (!response.ok) return false;
    const started = (await response.json()) as { url?: unknown };
    if (typeof started.url !== "string" || !started.url) return false;
    window.location.assign(started.url);
    return true;
  } catch {
    return false;
  }
}
