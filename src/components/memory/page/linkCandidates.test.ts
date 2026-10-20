// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Page } from "../../../lib/tauri";
import { linkCompletionMatches } from "../editor/wikiLinkCompletion";
import { LINK_CANDIDATE_BATCH, LINK_CANDIDATE_CAP, loadLinkCandidates } from "./linkCandidates";

const listPagesExplicitBrowse = vi.hoisted(() => vi.fn());
vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPagesExplicitBrowse,
}));

const page = (id: string, title: string): Page => ({ id, title, status: "active", space: "work" }) as Page;

/** A daemon holding `pages`, answering each offset/limit window. */
function serve(pages: Page[]) {
  listPagesExplicitBrowse.mockImplementation(async (_status: string, _space: string, limit: number, offset: number) =>
    pages.slice(offset, offset + limit));
}

const numbered = (count: number) => Array.from({ length: count }, (_, i) => page(`p${i}`, `Page ${i}`));

describe("loadLinkCandidates", () => {
  beforeEach(() => listPagesExplicitBrowse.mockReset());

  it("reads past the first batch, so a title shared beyond it is not offered as unique", async () => {
    const filler = Array.from({ length: LINK_CANDIDATE_BATCH }, (_, i) => page(`f${i}`, `Filler ${i}`));
    serve([page("early", "Roadmap"), ...filler, page("late", "roadmap")]);

    const pages = await loadLinkCandidates("work");

    expect(pages).toHaveLength(LINK_CANDIDATE_BATCH + 2);
    expect(listPagesExplicitBrowse).toHaveBeenNthCalledWith(1, "active", "work", LINK_CANDIDATE_BATCH, 0);
    expect(listPagesExplicitBrowse).toHaveBeenNthCalledWith(2, "active", "work", LINK_CANDIDATE_BATCH, LINK_CANDIDATE_BATCH);
    const notes = pages!.map((entry) => ({ id: entry.id, title: entry.title, canUseAsSource: true }));
    expect(linkCompletionMatches(notes, "roadmap")).toEqual([]);
  });

  it("asks the daemon for the pages in no Space when the page has none", async () => {
    serve([page("a", "Alpha")]);
    await loadLinkCandidates(null);
    expect(listPagesExplicitBrowse).toHaveBeenCalledWith("active", "uncategorized", LINK_CANDIDATE_BATCH, 0);
  });

  it("offers every page of a Space exactly at the cap", async () => {
    serve(numbered(LINK_CANDIDATE_CAP));
    expect(await loadLinkCandidates("work")).toHaveLength(LINK_CANDIDATE_CAP);
  });

  it("offers nothing when the Space is larger than it will read", async () => {
    serve(numbered(LINK_CANDIDATE_CAP + 1));
    expect(await loadLinkCandidates("work")).toBeNull();
    expect(listPagesExplicitBrowse).toHaveBeenCalledTimes(LINK_CANDIDATE_CAP / LINK_CANDIDATE_BATCH + 1);
  });
});
