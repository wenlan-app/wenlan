// SPDX-License-Identifier: AGPL-3.0-only
import { listPagesExplicitBrowse, listSpaces, type Page } from "../../../lib/tauri";
import { pageTitleKey, trimLikeDaemon } from "../editor/pageTitleKey";
import type { LinkCompletionNote } from "../editor/wikiLinkCompletion";

/** Pages read per request while loading the `[[` menu. */
export const LINK_CANDIDATE_BATCH = 500;
/** Past this many pages in one Space the menu stops reading and offers nothing. */
export const LINK_CANDIDATE_CAP = 10_000;

/**
 * Every active page in `space` (null: the pages in no Space), read in batches
 * so the `[[` menu sees each title the daemon compares a link against. A title
 * looks unique only when no other page in the Space shares it, so a partial
 * list could offer a link the daemon then finds ambiguous. A Space larger than
 * the cap therefore answers null and the menu offers nothing.
 *
 * The person typing `[[` asked for these pages, so this is an explicit browse:
 * it lists the pages the Wiki shows, including ones a background read hides,
 * which the daemon still resolves a link against.
 */
export async function loadLinkCandidates(space: string | null): Promise<Page[] | null> {
  // The daemon reads the reserved name "uncategorized" as the pages in no Space.
  const scope = space ?? "uncategorized";
  const byId = new Map<string, Page>();
  // One read past the cap tells "exactly at the cap" from "over it".
  for (let offset = 0; offset <= LINK_CANDIDATE_CAP; offset += LINK_CANDIDATE_BATCH) {
    const batch = await listPagesExplicitBrowse("active", scope, LINK_CANDIDATE_BATCH, offset);
    for (const page of batch) byId.set(page.id, page);
    if (byId.size > LINK_CANDIDATE_CAP) return null;
    if (batch.length < LINK_CANDIDATE_BATCH) return [...byId.values()];
  }
  return null;
}

/** The pages of one Space the `[[` menu read; `pages` is null when the Space is over the cap. */
export interface LinkCandidateSpace { name: string | null; pages: Page[] | null }

/**
 * The `[[` menu's pages: those in `space` (the page's own, null for no Space)
 * and those in every named Space, which `[[Space/Title]]` reaches. Pages in no
 * Space are never reachable from another Space, so they are read only as the
 * own Space.
 */
export async function loadAllLinkCandidates(space: string | null): Promise<LinkCandidateSpace[]> {
  const names = (await listSpaces()).map((entry) => entry.name);
  const scopes = [space, ...names.filter((name) => name !== space)];
  return Promise.all(scopes.map(async (name) => ({ name, pages: await loadLinkCandidates(name) })));
}

type TitleOwners = Map<string, string | null>;

/** Folded title to its one note, or null when two notes share it; entities are never link targets. */
function titleOwners(pages: readonly Page[]): TitleOwners {
  const owners: TitleOwners = new Map();
  for (const page of pages) {
    if (page.status !== "active" || page.creation_kind === "entity") continue;
    const key = pageTitleKey(page.title);
    owners.set(key, owners.has(key) ? null : page.id);
  }
  return owners;
}

/**
 * The note the daemon links `label` to from a page in `own`, mirroring its
 * resolver: a title in the own Space wins, even one two notes share; otherwise
 * the longest Space-name prefix that names one Space and a note in it.
 * `undefined` when the answer depends on a Space too large to read.
 */
function resolveLabel(
  label: string,
  own: TitleOwners,
  spaces: ReadonlyMap<string, TitleOwners | null | undefined>,
): string | null | undefined {
  const ownOwner = own.get(pageTitleKey(label));
  if (ownOwner !== undefined) return ownOwner;
  for (let at = label.lastIndexOf("/"); at >= 0; at = at === 0 ? -1 : label.lastIndexOf("/", at - 1)) {
    const title = trimLikeDaemon(label.slice(at + 1));
    if (!title) continue;
    const owners = spaces.get(pageTitleKey(label.slice(0, at)));
    if (owners === undefined) continue;
    if (owners === null) return undefined;
    const owner = owners.get(pageTitleKey(title));
    if (owner) return owner;
  }
  return null;
}

/**
 * The notes the `[[` menu offers to a page in `ownSpace`: each a unique title
 * in its Space whose inserted link the daemon resolves back to that very note.
 * Notes in another Space link as `Space/Title`.
 */
export function linkNotesFor(options: {
  page: Pick<Page, "id" | "creation_kind" | "basis">;
  ownSpace: string | null;
  spaces: readonly LinkCandidateSpace[];
}): LinkCompletionNote[] {
  const { page, ownSpace } = options;
  const own = options.spaces.find((entry) => entry.name === ownSpace);
  if (!own?.pages) return [];
  const ownOwners = titleOwners(own.pages);
  // Folded Space name to its title owners: null over the cap, undefined when two names fold together.
  const bySpaceKey = new Map<string, TitleOwners | null | undefined>();
  for (const entry of options.spaces) {
    if (entry.name === null) continue;
    const key = pageTitleKey(entry.name);
    bySpaceKey.set(key, bySpaceKey.has(key) ? undefined : entry.pages && titleOwners(entry.pages));
  }
  const reachable = new Map([...bySpaceKey].filter(([, owners]) => owners !== undefined));
  const taken = new Set((page.basis ?? []).map((entry) => entry.page_id));
  const canRely = page.creation_kind !== "entity";
  const notes: LinkCompletionNote[] = [];
  for (const entry of options.spaces) {
    if (!entry.pages) continue;
    const foreign = entry.name !== ownSpace;
    if (foreign && entry.name === null) continue;
    for (const candidate of entry.pages) {
      if (candidate.id === page.id || candidate.status !== "active" || candidate.creation_kind === "entity") continue;
      if ((candidate.space ?? null) !== entry.name) continue;
      const linkText = foreign ? `${trimLikeDaemon(entry.name!)}/${trimLikeDaemon(candidate.title)}` : trimLikeDaemon(candidate.title);
      if (resolveLabel(linkText, ownOwners, reachable) !== candidate.id) continue;
      notes.push({
        id: candidate.id,
        title: candidate.title,
        canUseAsSource: canRely && !taken.has(candidate.id),
        ...(foreign ? { space: entry.name!, linkText } : {}),
      });
    }
  }
  return notes;
}
