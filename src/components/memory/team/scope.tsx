// SPDX-License-Identifier: AGPL-3.0-only
// Which side of the library the window shows: the person's Private pages, or
// one team's. One at a time, picked at the top left, as local-first apps with
// both (Anytype, AppFlowy, Obsidian) do. A computer on no team has no switch
// and sees no trace of teams: Private is then the whole library.
//
// The switch is the window's only. What an AI tool may read stays that tool's
// own setting; it never follows the last side someone clicked.
import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { useShares } from "../sharing/sharing";
import { useTeamStatus } from "../sharing/team";
import { spacesOfTeam, teamGroups, type TeamGroup } from "./TeamPage";

export { spacesOfTeam };

const STORAGE_KEY = "wenlan-scope";
const PRIVATE = "private";

export type ScopeState = {
  /** The teams this computer is on; none hides the switch. */
  readonly teams: readonly TeamGroup[];
  /** The team shown, or null for Private. */
  readonly team: TeamGroup | null;
  readonly setScope: (key: string) => void;
  /** Every Space that belongs to a team, for keeping Private free of them. */
  readonly teamSpaces: ReadonlySet<string>;
  /** Whether a page or Space in `space` belongs on the side shown. */
  readonly inScope: (space: string | null | undefined) => boolean;
  /** The shown team's Spaces this person may write in; empty on Private. */
  readonly teamWritable: readonly string[];
};

function readStored(): string {
  try {
    return window.localStorage.getItem(STORAGE_KEY) ?? PRIVATE;
  } catch {
    return PRIVATE;
  }
}

function store(key: string) {
  try {
    window.localStorage.setItem(STORAGE_KEY, key);
  } catch {
    // Remembering the side is a convenience; the window works without it.
  }
}

export function useScopeState(): ScopeState {
  const shares = useShares();
  const status = useTeamStatus();
  const teams = useMemo(() => teamGroups(shares.data, status.data), [shares.data, status.data]);
  const [chosen, setChosen] = useState(readStored);
  const team = teams.find((group) => group.key === chosen) ?? null;
  const setScope = useCallback((key: string) => {
    setChosen(key);
    store(key);
  }, []);
  const hostedSpaces = status.data?.spaces;
  const teamSpaces = useMemo(() => new Set(teams.flatMap((group) => spacesOfTeam(group, hostedSpaces))), [teams, hostedSpaces]);
  const shown = useMemo(() => new Set(team ? spacesOfTeam(team, hostedSpaces) : []), [team, hostedSpaces]);
  const inScope = useCallback(
    (space: string | null | undefined) => (team ? Boolean(space && shown.has(space)) : !(space && teamSpaces.has(space))),
    [team, shown, teamSpaces],
  );
  const teamWritable = useMemo(() => {
    if (!team) return [];
    // The owner writes in every Space of the team; a member where not read only.
    if (team.hosted) return spacesOfTeam(team, hostedSpaces);
    return team.shares.filter((share) => !share.read_only).map((share) => share.target);
  }, [team, hostedSpaces]);
  return { teams, team, setScope, teamSpaces, inScope, teamWritable };
}

const everything: ScopeState = {
  teams: [],
  team: null,
  setScope: () => {},
  teamSpaces: new Set(),
  inScope: () => true,
  teamWritable: [],
};

const ScopeContext = createContext<ScopeState>(everything);

export function ScopeProvider({ value, children }: { readonly value: ScopeState; readonly children: ReactNode }) {
  return <ScopeContext.Provider value={value}>{children}</ScopeContext.Provider>;
}

/** The side the window shows. Outside the shell (tests, the review flavor)
 *  it is the whole library. */
export function useScope(): ScopeState {
  return useContext(ScopeContext);
}

export const SCOPE_PRIVATE = PRIVATE;
