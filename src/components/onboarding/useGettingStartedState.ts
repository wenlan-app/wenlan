// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useSyncExternalStore } from "react";
import { useQuery } from "@tanstack/react-query";
import {
  listOnboardingMilestones,
  type AgentConnection,
  type MilestoneRecord,
  type Page,
} from "../../lib/tauri";
import { OWN_IDENTITY, toolFamilyOf } from "../../lib/agents";
import { useLiveAgents } from "../connect/connectionState";
import { useLibraryInventory } from "../memory/pages/useLibraryInventory";

/** Per-viewer convenience only: losing it just shows the card again. */
const DISMISSED_KEY = "onboarding:gettingStartedDismissed";
/** "Skip for now" on the pages step, for someone with nothing to bring in.
 *  Cleared once material arrives, so the step comes back with its button. */
const PAGES_SKIPPED_KEY = "onboarding:pagesStepSkipped";
/** A library with this many pages written by the user or from their material
 *  is past first use; existing users upgrading into this card would read it as
 *  noise. Copies of brought-in files do not count (`libraryPagesForEstablished`),
 *  or one imported folder would hide the card from a new user. */
export const ESTABLISHED_LIBRARY_PAGES = 10;

export type GettingStartedStepId = "note" | "tool" | "pages" | "recall";

/** The path most people take: bring material in and have it turned into
 *  pages (one step, so someone with nothing to bring is not left with an
 *  empty one), then ask about them. Writing a page by hand is last. */
export const GETTING_STARTED_ORDER: readonly GettingStartedStepId[] = ["tool", "pages", "recall", "note"];

export interface GettingStartedProgress {
  readonly note: boolean;
  readonly tool: boolean;
  /** A page written from material, or the user chose to skip the step. */
  readonly pages: boolean;
  readonly recall: boolean;
}

/** A draft lives only in the editor until the user leaves it; recall cannot
 *  see it, so it does not count as a saved note. */
export function publishedPages(pages: readonly Page[]): Page[] {
  return pages.filter((page) => page.status !== "draft");
}

/** Copies of files the user brought in, and pages Wenlan made, are not the
 *  user writing a page. */
const NOT_WRITTEN_BY_USER = new Set(["source", "imported", "distilled", "entity"]);

/** Pages the user wrote themselves. */
export function writtenPages(pages: readonly Page[]): Page[] {
  return publishedPages(pages).filter((page) => !NOT_WRITTEN_BY_USER.has(page.creation_kind ?? ""));
}

/** Pages worth asking an AI tool about: ones written from material first, then
 *  the user's own. A copy of a brought-in file is not one; its title is the
 *  file's (e.g. "ISP Outage Runbook (section_h1)"). */
export function askablePages(pages: readonly Page[]): Page[] {
  const published = publishedPages(pages);
  return [
    ...published.filter((page) => page.creation_kind === "distilled"),
    ...writtenPages(published),
  ];
}

/** Pages that say a library is past first use: every page except copies of
 *  brought-in files and entity pages. */
export function libraryPagesForEstablished(pages: readonly Page[]): Page[] {
  return pages.filter((page) => !COPIED_OR_ENTITY.has(page.creation_kind ?? ""));
}
const COPIED_OR_ENTITY = new Set(["source", "imported", "entity"]);

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
  milestones,
  pagesSkipped = false,
}: {
  pages: readonly Page[];
  agents: readonly AgentConnection[];
  milestones: readonly MilestoneRecord[];
  pagesSkipped?: boolean;
}): GettingStartedProgress {
  return {
    note: writtenPages(pages).length > 0,
    tool: connectedToolFamilies(agents).length > 0,
    pages: pagesSkipped || pages.some((page) => page.creation_kind === "distilled" && page.status !== "draft"),
    recall: milestones.some((milestone) => milestone.id === "first-recall"),
  };
}

// The dismissed flag is read by the card and by Main (which keeps the
// directory hidden while the card is open), so each flag lives in one store
// every consumer subscribes to instead of per-component state.
function flagStore(key: string) {
  const listeners = new Set<() => void>();
  /** Set only when storage rejected the last write, so the choice still
   *  holds for this session. */
  let inMemory: boolean | null = null;
  return {
    read(): boolean {
      try {
        const stored = localStorage.getItem(key) === "1";
        return inMemory ?? stored;
      } catch {
        return inMemory ?? false;
      }
    },
    write(value: boolean) {
      try {
        if (value) localStorage.setItem(key, "1");
        else localStorage.removeItem(key);
        inMemory = null;
      } catch {
        // Storage can be unavailable; the choice still holds for this session.
        inMemory = value;
      }
      listeners.forEach((listener) => listener());
    },
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
  };
}

const dismissedFlag = flagStore(DISMISSED_KEY);
const pagesSkippedFlag = flagStore(PAGES_SKIPPED_KEY);

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
  readonly pagesSkipped: boolean;
  readonly setPagesSkipped: (value: boolean) => void;
  readonly pages: readonly Page[];
  readonly agents: AgentConnection[];
  /** Only writing a page by hand is left, and there are pages to read: the
   *  card shrinks to one line and the pages take the home. */
  readonly compact: boolean;
}

/** Writing a page by hand is the one step left, and the library already has
 *  pages worth reading. For many people that step is not the goal, so the
 *  card stops taking the screen. */
export function onlyOwnPageLeft(progress: GettingStartedProgress | null, pages: readonly Page[]): boolean {
  return progress !== null && progress.tool && progress.pages && progress.recall && !progress.note
    && askablePages(pages).length > 0;
}

export function useGettingStartedState(): GettingStartedState {
  const { pages, ready } = useLibraryInventory();
  const established = libraryPagesForEstablished(pages).length >= ESTABLISHED_LIBRARY_PAGES;
  const checking = !established;
  const agentsQuery = useLiveAgents(undefined, checking);
  // Same key as useMilestones, read without its event listener: the card only
  // needs whether a recall has happened, and the shared cache keeps it fresh.
  const milestonesQuery = useQuery({
    queryKey: ["onboarding-milestones"],
    queryFn: listOnboardingMilestones,
    staleTime: 30_000,
    enabled: checking,
  });
  const dismissed = useSyncExternalStore(dismissedFlag.subscribe, dismissedFlag.read, () => false);
  const setDismissed = useCallback((value: boolean) => dismissedFlag.write(value), []);
  const pagesSkipped = useSyncExternalStore(pagesSkippedFlag.subscribe, pagesSkippedFlag.read, () => false);
  const setPagesSkipped = useCallback((value: boolean) => pagesSkippedFlag.write(value), []);
  const flags = { dismissed, setDismissed, pagesSkipped, setPagesSkipped };

  const loaded = ready && agentsQuery.isSuccess && milestonesQuery.isSuccess;
  if (!loaded || established) {
    return { visible: false, progress: null, doneCount: 0, ...flags, pages, agents: [], compact: false };
  }
  const progress = gettingStartedProgress({
    pages,
    agents: agentsQuery.data,
    milestones: milestonesQuery.data,
    pagesSkipped,
  });
  const doneCount = GETTING_STARTED_ORDER.filter((id) => progress[id]).length;
  const visible = doneCount < GETTING_STARTED_ORDER.length;
  const shown = visible ? progress : null;
  return {
    visible,
    progress: shown,
    doneCount,
    ...flags,
    pages,
    agents: agentsQuery.data,
    compact: onlyOwnPageLeft(shown, pages),
  };
}
