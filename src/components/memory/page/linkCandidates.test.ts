// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Page } from "../../../lib/tauri";
import { linkCompletionMatches } from "../editor/wikiLinkCompletion";
import { LINK_CANDIDATE_BATCH, LINK_CANDIDATE_CAP, loadLinkCandidates } from "./linkCandidates";

const listPages = vi.hoisted(() => vi.fn());
vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPages,
}));

const page = (id: string, title: string): Page => ({ id, title, status: "active", space: "work" }) as Page;

/** A daemon holding `pages`, answering each offset/limit window. */
function serve(pages: Page[]) {
  listPages.mockImplementation(async (_status: string, _space: string, limit: number, offset: number) =>
    pages.slice(offset, offset + limit));
}

describe("loadLinkCandidates", () => {
  beforeEach(() => listPages.mockReset());

  it("reads past the first batch, so a title shared beyond it is not offered as unique", async () => {
    const filler = Array.from({ length: LINK_CANDIDATE_BATCH }, (_, i) => page(`f${i}`, `Filler ${i}`));
    serve([page("early", "Roadmap"), ...filler, page("late", "roadmap")]);

    const pages = await loadLinkCandidates("work");

    expect(pages).toHaveLength(LINK_CANDIDATE_BATCH + 2);
    expect(listPages).toHaveBeenNthCalledWith(1, "active", "work", LINK_CANDIDATE_BATCH, 0);
    expect(listPages).toHaveBeenNthCalledWith(2, "active", "work", LINK_CANDIDATE_BATCH, LINK_CANDIDATE_BATCH);
    const notes = pages!.map((entry) => ({ id: entry.id, title: entry.title, canUseAsSource: true }));
    expect(linkCompletionMatches(notes, "roadmap")).toEqual([]);
  });

  it("asks the daemon for the pages in no Space when the page has none", async () => {
    serve([page("a", "Alpha")]);
    await loadLinkCandidates(null);
    expect(listPages).toHaveBeenCalledWith("active", "uncategorized", LINK_CANDIDATE_BATCH, 0);
  });

  it("offers nothing when the Space is larger than it will read", async () => {
    listPages.mockImplementation(async (_status: string, _space: string, limit: number, offset: number) =>
      Array.from({ length: limit }, (_, i) => page(`p${offset + i}`, `Page ${offset + i}`)));
    expect(await loadLinkCandidates("work")).toBeNull();
    expect(listPages).toHaveBeenCalledTimes(LINK_CANDIDATE_CAP / LINK_CANDIDATE_BATCH);
  });
});
