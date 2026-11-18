// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { getSpaceFolders, type Page } from "../../../lib/tauri";
import { RECENT_PAGES_CHANGED_EVENT, readRecentPageHistory } from "../../../lib/recentPages";
import { useScope } from "../team/scope";
import { EXPLICIT_BROWSE_QUERY_POLICY, listAllActivePages, listAllActivePagesExplicitBrowse, listAllDraftPages, listAllDraftPagesExplicitBrowse } from "./listAllPages";
import { collectPageInventory } from "./pageInventory";
import { PAGE_SECTIONS, pageSection, spaceLayout, type PageSection, type SpaceLayout } from "./pageSections";
import { SPACE_FOLDERS_QUERY_KEY } from "./SpaceFoldersPrompt";

export type SectionPages = {
  /** Every page, whichever side it is on. */
  readonly allPages: readonly Page[];
  /** Every page of the side shown, newest change first. */
  readonly pages: readonly Page[];
  readonly bySection: Readonly<Record<PageSection, readonly Page[]>>;
  /** Which folders are a Space's own; their names are not section names. */
  readonly layout: SpaceLayout;
  /** The one section a page is counted and listed in. */
  readonly sectionOf: (page: Page) => PageSection;
  /** Whether both lists have answered; counts are not shown before. */
  readonly loaded: boolean;
  readonly pending: boolean;
  readonly failed: boolean;
  readonly retry: () => void;
};

/** The person's pages, split into Docs, Skills and Rules. `browsing` is the
 *  list page, where the person asked to see them; without it the read is the
 *  quiet one the left side's counts use. Both share the tree's query keys. */
export function useSectionPages(browsing = false): SectionPages {
  const active = useQuery({ queryKey: browsing ? ["pages", "active"] : ["pages", "inventory", "passive", "active"], queryFn: browsing ? listAllActivePagesExplicitBrowse : listAllActivePages, ...(browsing ? EXPLICIT_BROWSE_QUERY_POLICY : { staleTime: 30_000 }) });
  const drafts = useQuery({ queryKey: browsing ? ["pages", "draft"] : ["pages", "inventory", "passive", "draft"], queryFn: browsing ? listAllDraftPagesExplicitBrowse : listAllDraftPages, ...(browsing ? EXPLICIT_BROWSE_QUERY_POLICY : { staleTime: 30_000 }) });
  const scope = useScope();
  const allPages = useMemo(() => collectPageInventory(active.data ?? [], drafts.data ?? []), [active.data, drafts.data]);
  const pages = useMemo(() => allPages.filter((page) => scope.inScope(page.space)), [allPages, scope]);
  // The daemon's own setting says whether Spaces keep folders of their own.
  const spaceFolders = useQuery({ queryKey: SPACE_FOLDERS_QUERY_KEY, queryFn: getSpaceFolders, retry: false, staleTime: Infinity });
  const layout = useMemo(() => spaceLayout(allPages, spaceFolders.data === true, scope.teamSpaces), [allPages, spaceFolders.data, scope.teamSpaces]);
  const sectionOf = useCallback((page: Page) => pageSection(page, layout), [layout]);
  const bySection = useMemo(() => {
    const split: Record<PageSection, Page[]> = { docs: [], skills: [], rules: [] };
    for (const page of pages) split[sectionOf(page)].push(page);
    return split;
  }, [pages, sectionOf]);
  return {
    allPages,
    pages,
    bySection,
    layout,
    sectionOf,
    loaded: active.isSuccess && drafts.isSuccess,
    pending: active.isPending || drafts.isPending,
    failed: active.isError || drafts.isError,
    retry: () => { void active.refetch(); void drafts.refetch(); },
  };
}

export function sectionCounts(sections: SectionPages): Partial<Record<PageSection, number>> {
  if (!sections.loaded) return {};
  return Object.fromEntries(PAGE_SECTIONS.map((section) => [section, sections.bySection[section].length]));
}

/** The pages opened lately on this computer that still exist, newest first,
 *  each with its section. Redraws when any editor records a visit. */
export function useRecentSectionPages(pages: readonly Page[], limit: number, sectionOf: (page: Page) => PageSection = pageSection): readonly { id: string; title: string; section: PageSection; page: Page }[] {
  const [tick, setTick] = useState(0);
  useEffect(() => {
    const redraw = () => setTick((value) => value + 1);
    window.addEventListener(RECENT_PAGES_CHANGED_EVENT, redraw);
    return () => window.removeEventListener(RECENT_PAGES_CHANGED_EVENT, redraw);
  }, []);
  return useMemo(() => {
    const byId = new Map(pages.filter((page) => page.status === "active").map((page) => [page.id, page] as const));
    const recent: { id: string; title: string; section: PageSection; page: Page }[] = [];
    for (const entry of readRecentPageHistory().entries) {
      const page = byId.get(entry.id);
      if (!page) continue;
      recent.push({ id: page.id, title: page.title, section: sectionOf(page), page });
      if (recent.length === limit) break;
    }
    return recent;
    // `tick` stands for the stored history, which has no identity of its own.
  }, [pages, limit, tick, sectionOf]);
}
