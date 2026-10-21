// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Page } from "../../../lib/tauri";
import { linkCompletionMatches } from "../editor/wikiLinkCompletion";
import { LINK_CANDIDATE_BATCH, LINK_CANDIDATE_CAP, linkNotesFor, loadAllLinkCandidates, loadLinkCandidates, type LinkCandidateSpace } from "./linkCandidates";

const listPagesExplicitBrowse = vi.hoisted(() => vi.fn());
const listSpaces = vi.hoisted(() => vi.fn());
vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listPagesExplicitBrowse,
  listSpaces,
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

describe("loadAllLinkCandidates", () => {
  beforeEach(() => { listPagesExplicitBrowse.mockReset(); listSpaces.mockReset(); });

  it("reads the page's own Space first, then every named Space once", async () => {
    listSpaces.mockResolvedValue([{ name: "home" }, { name: "work" }]);
    listPagesExplicitBrowse.mockImplementation(async (_status: string, space: string) => [page(`${space}-1`, space)]);
    const spaces = await loadAllLinkCandidates("work");
    expect(spaces.map((entry) => entry.name)).toEqual(["work", "home"]);
    expect(listPagesExplicitBrowse.mock.calls.map((call) => call[1])).toEqual(["work", "home"]);
  });

  it("reads the pages in no Space only for a page that has none", async () => {
    listSpaces.mockResolvedValue([{ name: "home" }]);
    listPagesExplicitBrowse.mockResolvedValue([]);
    expect((await loadAllLinkCandidates(null)).map((entry) => entry.name)).toEqual([null, "home"]);
  });
});

describe("linkNotesFor", () => {
  const at = (space: string | null, id: string, title: string, extra: Partial<Page> = {}): Page =>
    ({ id, title, status: "active", space, ...extra }) as Page;
  const self = { id: "self", creation_kind: "user" as Page["creation_kind"], basis: [] };
  const notesFor = (ownSpace: string | null, spaces: LinkCandidateSpace[]) =>
    linkNotesFor({ page: self, ownSpace, spaces }).map((note) => [note.id, note.linkText ?? note.title, note.space ?? null]);

  it("offers the own Space by title and other Spaces as Space/Title", () => {
    expect(notesFor("work", [
      { name: "work", pages: [at("work", "self", "Me"), at("work", "w1", "Plan")] },
      { name: "home", pages: [at("home", "h1", "Plan"), at("home", "h2", "Trip", { creation_kind: "entity" } as Partial<Page>)] },
    ])).toEqual([["w1", "Plan", null], ["h1", "home/Plan", "home"]]);
  });

  it("leaves out a note whose Space/Title link the daemon would send elsewhere", () => {
    expect(notesFor("work", [
      // The own Space's "home/Plan" wins over the home Space's "Plan".
      { name: "work", pages: [at("work", "w1", "home/Plan")] },
      { name: "home", pages: [at("home", "h1", "Plan"), at("home", "h2", "Trip"), at("home", "h3", "Trip")] },
      // "a/b" is the longest prefix of "a/b/C", so a's "b/C" is never reached.
      { name: "a", pages: [at("a", "a1", "b/C")] },
      { name: "a/b", pages: [at("a/b", "ab1", "C")] },
    ])).toEqual([["w1", "home/Plan", null], ["ab1", "a/b/C", "a/b"]]);
  });

  it("offers nothing from a Space whose name another Space shares, or whose pages it could not read", () => {
    expect(notesFor("work", [
      { name: "work", pages: [] },
      { name: "Home", pages: [at("Home", "h1", "Plan")] },
      { name: "home", pages: [at("home", "h2", "Notes")] },
      { name: "big", pages: null },
    ])).toEqual([]);
  });

  it("does not reach a page in no Space from a named Space", () => {
    expect(notesFor("work", [
      { name: "work", pages: [] },
      { name: null, pages: [at(null, "u1", "Loose")] },
    ])).toEqual([]);
    expect(notesFor(null, [
      { name: null, pages: [at(null, "u1", "Loose")] },
      { name: "home", pages: [at("home", "h1", "Plan")] },
    ])).toEqual([["u1", "Loose", null], ["h1", "home/Plan", "home"]]);
  });

  it("offers nothing when the page's own Space was too large to read", () => {
    expect(notesFor("work", [{ name: "work", pages: null }, { name: "home", pages: [at("home", "h1", "Plan")] }])).toEqual([]);
  });

  it("offers a note in another Space as a source unless it already is one", () => {
    const notes = linkNotesFor({
      page: { ...self, basis: [{ page_id: "h1" }] as Page["basis"] },
      ownSpace: "work",
      spaces: [{ name: "work", pages: [] }, { name: "home", pages: [at("home", "h1", "Plan"), at("home", "h2", "Trip")] }],
    });
    expect(notes.map((note) => [note.id, note.canUseAsSource])).toEqual([["h1", false], ["h2", true]]);
  });
});
