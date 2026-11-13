// SPDX-License-Identifier: AGPL-3.0-only
// Team server helpers shared by the members dialog, the share box, the join
// box and Settings › Team.
import { useQuery } from "@tanstack/react-query";
import { teamFirewall, teamStatus, type TeamMember } from "../../../lib/tauri";

export const TEAM_DOCS_URL = "https://github.com/wenlan-app/wenlan/blob/main/docs/team-server.md";
const TEAM_URL_KEY = "wenlan-team-url";

export function rememberedTeamUrl(): string {
  try {
    return localStorage.getItem(TEAM_URL_KEY) ?? "";
  } catch {
    return "";
  }
}

export function rememberTeamUrl(url: string) {
  try {
    localStorage.setItem(TEAM_URL_KEY, url);
  } catch {
    // Remembering the address is a convenience only.
  }
}

/** The address a member would reach the server at, when the listener names a
 *  real one. `0.0.0.0` and `::` only say "every network", not where to go. */
export function addressFromListening(listening: string | null | undefined): string {
  if (!listening) return "";
  const host = listening.startsWith("[") ? listening.slice(1, listening.indexOf("]")) : listening.replace(/:\d+$/, "");
  if (!host || host === "0.0.0.0" || host === "::") return "";
  return `http://${listening}`;
}

export function prefillTeamUrl(listening: string | null | undefined): string {
  return rememberedTeamUrl() || addressFromListening(listening);
}

/** A server's address as people read it: no scheme, no trailing slash. */
export function hostName(url: string): string {
  return url.replace(/^[a-z][a-z0-9+.-]*:\/\//i, "").replace(/\/+$/, "");
}

export function activeMembers(members: readonly TeamMember[]): TeamMember[] {
  return members.filter((member) => !member.removed_at);
}

export const TEAM_KEY = ["team"] as const;

/** The team server's people and hosted Spaces. Only a computer that runs a
 *  team server answers; elsewhere it reads as "off". */
export function useTeamStatus({ enabled = true }: { readonly enabled?: boolean } = {}) {
  return useQuery({ queryKey: TEAM_KEY, queryFn: teamStatus, retry: false, enabled });
}

export const TEAM_FIREWALL_KEY = ["team-firewall"] as const;

/** On Windows, whether teammates get through Windows Firewall to this
 *  computer's team; `null` elsewhere. Asking takes Windows a few seconds,
 *  so it is asked only while the team is on, and again when the window
 *  comes back to the front: after Windows' own alert is answered, or on
 *  another network. */
export function useTeamFirewall(enabled: boolean) {
  return useQuery({ queryKey: TEAM_FIREWALL_KEY, queryFn: teamFirewall, retry: false, enabled, staleTime: 5_000, refetchOnWindowFocus: true });
}

/** Whether `name` is the same person as `member`: trimmed, any letter case. */
export function sameName(a: string, b: string): boolean {
  return a.trim().toLocaleLowerCase() === b.trim().toLocaleLowerCase();
}
