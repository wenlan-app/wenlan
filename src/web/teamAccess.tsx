// SPDX-License-Identifier: AGPL-3.0-only
import { createContext, useContext, type ReactNode } from "react";
import { isTeamMemberMode, type TeamMe } from "./team";

export interface TeamAccess {
  /** True in the browser bundle a team server serves at `/app`. */
  member: boolean;
  /** Whether this member may change pages of `space`. Always true outside team mode. */
  canEdit: (space: string | null | undefined) => boolean;
  /** The Spaces this member may write in, in the server's order. Empty outside team mode. */
  writable: readonly string[];
}

const OUTSIDE: TeamAccess = { member: false, canEdit: () => true, writable: [] };

const TeamAccessContext = createContext<TeamAccess>(OUTSIDE);

export function teamAccessFor(me: TeamMe): TeamAccess {
  const access = new Map(me.spaces.map((entry) => [entry.space, entry.access]));
  return {
    member: true,
    canEdit: (space) => !!space && access.get(space) === "write",
    writable: me.spaces.filter((entry) => entry.access === "write").map((entry) => entry.space),
  };
}

export function TeamAccessProvider({ value, children }: { value: TeamAccess; children: ReactNode }) {
  return <TeamAccessContext.Provider value={value}>{children}</TeamAccessContext.Provider>;
}

/**
 * The member's access. In team mode with no provider (which should not
 * happen), nothing may be changed.
 */
export function useTeamAccess(): TeamAccess {
  const value = useContext(TeamAccessContext);
  if (value === OUTSIDE && isTeamMemberMode()) return { member: true, canEdit: () => false, writable: [] };
  return value;
}
