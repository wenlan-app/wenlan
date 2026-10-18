// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  listOnboardingMilestones,
  listRegisteredSources,
  type AgentConnection,
  type MilestoneRecord,
  type Page,
  type RegisteredSource,
} from "../../lib/tauri";
import { OWN_IDENTITY, toolFamilyOf } from "../../lib/agents";
import { useLiveAgents } from "../connect/connectionState";
import { useLibraryInventory } from "../memory/pages/useLibraryInventory";

/** Per-viewer convenience only: losing it just shows the card again. */
const DISMISSED_KEY = "onboarding:gettingStartedDismissed";
/** A library with this many pages is past first use; existing users upgrading
 *  into this card would read it as noise. */
export const ESTABLISHED_LIBRARY_PAGES = 10;

export type GettingStartedStepId = "note" | "tool" | "sources" | "recall";

export const GETTING_STARTED_ORDER: readonly GettingStartedStepId[] = ["note", "tool", "sources", "recall"];

export interface GettingStartedProgress {
  readonly note: boolean;
  readonly tool: boolean;
  readonly sources: boolean;
  readonly recall: boolean;
}

/** A draft lives only in the editor until the user leaves it; recall cannot
 *  see it, so it does not count as a saved note. */
export function publishedPages(pages: readonly Page[]): Page[] {
  return pages.filter((page) => page.status !== "draft");
}

/** The AI tools the daemon knows, Wenlan's own identities excluded.
 *  Connecting a tool from the app registers it (`POST /api/agents`), so a row
 *  is the user's own act of connecting; the step does not also wait for the
 *  tool's first call, which happens outside the app and reads as "I did it,
 *  why isn't it done". */
export function connectedToolFamilies(agents: readonly AgentConnection[]): string[] {
  const families = new Set<string>();
  for (const agent of agents) {
    const family = toolFamilyOf(agent);
    if (!OWN_IDENTITY.test(family)) families.add(family);
  }
  return [...families];
}

/** Each step is read from what the library holds now, not from a stored
 *  checkmark, so it stays true to what the user actually did. */
export function gettingStartedProgress({
  pages,
  agents,
  sources,
  milestones,
}: {
  pages: readonly Page[];
  agents: readonly AgentConnection[];
  sources: readonly RegisteredSource[];
  milestones: readonly MilestoneRecord[];
}): GettingStartedProgress {
  return {
    note: publishedPages(pages).length > 0,
    tool: connectedToolFamilies(agents).length > 0,
    sources: sources.length > 0
      || pages.some((page) => page.creation_kind === "source" || page.creation_kind === "imported"),
    recall: milestones.some((milestone) => milestone.id === "first-recall")
      || pages.some((page) => page.creation_kind === "distilled"),
  };
}

// The dismissed flag is read by the card and by Main (which keeps the
// directory hidden while the card is open), so it lives in one store both
// subscribe to instead of per-component state.
const dismissedListeners = new Set<() => void>();
/** Set only when storage rejected the last write, so the choice still holds
 *  for this session. */
let dismissedInMemory: boolean | null = null;

function readDismissed(): boolean {
  try {
    const stored = localStorage.getItem(DISMISSED_KEY) === "1";
    return dismissedInMemory ?? stored;
  } catch {
    return dismissedInMemory ?? false;
  }
}

function writeDismissed(value: boolean) {
  try {
    if (value) localStorage.setItem(DISMISSED_KEY, "1");
    else localStorage.removeItem(DISMISSED_KEY);
    dismissedInMemory = null;
  } catch {
    // Storage can be unavailable; the card still hides for this session.
    dismissedInMemory = value;
  }
  dismissedListeners.forEach((listener) => listener());
}

function subscribeDismissed(listener: () => void) {
  dismissedListeners.add(listener);
  return () => {
    dismissedListeners.delete(listener);
  };
}

/**
 * Whether the getting-started checklist is on the Wiki home, and how far along
 * it is. Shared by the card and by Main so both agree, over the same React
 * Query cache entries (a second consumer costs no extra request).
 *
 * `visible` stays true while the card is collapsed to its "show getting
 * started" line; `dismissed` says whether it is collapsed. It is false when
 * any read is missing or failed (the card would claim a step is undone when
 * it may not be), once the library is established, and once every step is done.
 */
export interface GettingStartedState {
  readonly visible: boolean;
  /** Null while the card is not visible. */
  readonly progress: GettingStartedProgress | null;
  readonly doneCount: number;
  readonly dismissed: boolean;
  readonly setDismissed: (value: boolean) => void;
  readonly pages: readonly Page[];
  readonly agents: AgentConnection[];
}

export function useGettingStartedState(): GettingStartedState {
  const { pages, ready } = useLibraryInventory();
  const established = pages.length >= ESTABLISHED_LIBRARY_PAGES;
  const checking = !established;
  const agentsQuery = useLiveAgents(undefined, checking);
  const sourcesQuery = useQuery({
    queryKey: ["registeredSources"],
    queryFn: listRegisteredSources,
    refetchOnWindowFocus: true,
    enabled: checking,
  });
  // Same key as useMilestones, read without its event listener: the card only
  // needs whether a recall has happened, and the shared cache keeps it fresh.
  const milestonesQuery = useQuery({
    queryKey: ["onboarding-milestones"],
    queryFn: listOnboardingMilestones,
    staleTime: 30_000,
    enabled: checking,
  });
  const dismissed = useSyncExternalStore(subscribeDismissed, readDismissed, () => false);
  const setDismissed = useCallback((value: boolean) => writeDismissed(value), []);

  const loaded = ready && agentsQuery.isSuccess && sourcesQuery.isSuccess && milestonesQuery.isSuccess;
  if (!loaded || established) {
    return { visible: false, progress: null, doneCount: 0, dismissed, setDismissed, pages, agents: [] };
  }
  const progress = gettingStartedProgress({
    pages,
    agents: agentsQuery.data,
    sources: sourcesQuery.data,
    milestones: milestonesQuery.data,
  });
  const doneCount = GETTING_STARTED_ORDER.filter((id) => progress[id]).length;
  const visible = doneCount < GETTING_STARTED_ORDER.length;
  return { visible, progress: visible ? progress : null, doneCount, dismissed, setDismissed, pages, agents: agentsQuery.data };
}
