// SPDX-License-Identifier: AGPL-3.0-only
import { listPagesExplicitBrowse, type Page } from "../../../lib/tauri";

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
