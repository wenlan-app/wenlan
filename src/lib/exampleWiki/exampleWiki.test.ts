// SPDX-License-Identifier: AGPL-3.0-only
import { invoke as tauriInvoke } from "@tauri-apps/api/core";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { toast } from "sonner";
import { buildExampleWiki, type ExampleLocale } from "./build";
import { ExampleWikiReadOnly, exampleInvoke } from "./invoke";
import { exampleWikiActive, setExampleWikiActive } from "./mode";
import { listSpaces } from "../tauri";
import { EXAMPLE_WIKI } from "./content";

vi.mock("sonner", () => ({ toast: vi.fn() }));

const LOCALES: ExampleLocale[] = ["en", "zh-Hant", "zh-Hans"];

describe("the example wiki's content", () => {
  it.each(LOCALES)("%s: nine pages in two spaces, every citation pointing at a memory it has", (locale) => {
    const wiki = buildExampleWiki(locale);
    expect(wiki.pages).toHaveLength(9);
    expect(new Set(wiki.pages.map((page) => page.space)).size).toBe(2);
    expect(wiki.spaces.map((space) => space.name).sort()).toEqual([...new Set(wiki.pages.map((page) => page.space))].sort());
    const memoryIds = new Set(wiki.memories.map((memory) => memory.source_id));
    for (const page of wiki.pages) {
      expect(page.citations?.length).toBeGreaterThan(0);
      for (const citation of page.citations ?? []) {
        expect(memoryIds.has(citation.locator)).toBe(true);
        expect(page.content).toContain(`[${citation.marker}]`);
      }
    }
  });

  it.each(LOCALES)("%s: shows at least one sentence its sources don't back, and every wiki link lands", (locale) => {
    const wiki = buildExampleWiki(locale);
    const unverified = wiki.pages.flatMap((page) => page.citations ?? []).filter((citation) => citation.status === "unverified");
    expect(unverified.length).toBeGreaterThan(0);
    for (const links of wiki.pageLinks.values()) {
      for (const link of links.outbound) expect(link.target_page_id).not.toBeNull();
    }
  });

  it("comes from at least five kinds of source", () => {
    expect(new Set(EXAMPLE_WIKI.sources.map((source) => source.kind)).size).toBeGreaterThanOrEqual(5);
  });

  it("tells the same story in every language: same pages, same order", () => {
    const [en, ...others] = LOCALES.map((locale) => buildExampleWiki(locale));
    for (const other of others) {
      expect(other.pages.map((page) => page.id)).toEqual(en!.pages.map((page) => page.id));
      expect(other.pages.map((page) => page.citations?.length)).toEqual(en!.pages.map((page) => page.citations?.length));
    }
  });
});

describe("commands while the example wiki is open", () => {
  beforeEach(() => {
    vi.mocked(tauriInvoke).mockReset();
    vi.mocked(toast).mockClear();
  });
  afterEach(() => setExampleWikiActive(false));

  it("answers library reads from the example, never from the daemon", async () => {
    const pages = await exampleInvoke<{ id: string }[]>("list_pages", {});
    expect(pages).toHaveLength(9);
    const page = await exampleInvoke<{ id: string } | null>("get_page", { id: pages[0]!.id });
    expect(page?.id).toBe(pages[0]!.id);
    expect(await exampleInvoke("list_refinements")).toEqual({ proposals: [] });
    expect(tauriInvoke).not.toHaveBeenCalled();
  });

  it("refuses anything that would change the library, and says so once", async () => {
    await expect(exampleInvoke("update_page", { id: "x", content: "y" })).rejects.toBeInstanceOf(ExampleWikiReadOnly);
    await expect(exampleInvoke("delete_page", { id: "x" })).rejects.toBeInstanceOf(ExampleWikiReadOnly);
    expect(tauriInvoke).not.toHaveBeenCalled();
    expect(toast).toHaveBeenCalledWith(expect.any(String), { id: "example-wiki-read-only" });
  });

  it("passes native composition lifecycle through without permitting library writes",async()=>{
    vi.mocked(tauriInvoke).mockResolvedValue(null);
    await exampleInvoke("set_editor_composing",{active:true});
    await exampleInvoke("set_editor_composing",{active:false});
    expect(tauriInvoke).toHaveBeenNthCalledWith(1,"set_editor_composing",{active:true});
    expect(tauriInvoke).toHaveBeenNthCalledWith(2,"set_editor_composing",{active:false});
    expect(toast).not.toHaveBeenCalled();
    await expect(exampleInvoke("archive_page",{id:"sample"})).rejects.toBeInstanceOf(ExampleWikiReadOnly);
    await expect(exampleInvoke("restore_page",{id:"sample"})).rejects.toBeInstanceOf(ExampleWikiReadOnly);
    expect(tauriInvoke).toHaveBeenCalledTimes(2);
  });

  it("still lets app commands through: profile, settings and connected tools", async () => {
    vi.mocked(tauriInvoke).mockResolvedValueOnce([]);
    await exampleInvoke("list_agents");
    expect(tauriInvoke).toHaveBeenCalledWith("list_agents", undefined);
  });

  it("routes the app's own invoke through the example only while it is open", async () => {
    vi.mocked(tauriInvoke).mockResolvedValue([]);
    setExampleWikiActive(true);
    expect(exampleWikiActive()).toBe(true);
    expect(await listSpaces()).toHaveLength(2);
    expect(tauriInvoke).not.toHaveBeenCalled();
    setExampleWikiActive(false);
    await listSpaces();
    expect(tauriInvoke).toHaveBeenCalled();
  });
});
