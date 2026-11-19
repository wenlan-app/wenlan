// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { getSpaceRules } from "../../../lib/tauri";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import type { SpaceRulesResponse } from "../../../lib/tauri";
import { NO_SAVED_RULES_PAGES, SAVED_RULES_PAGES_QUERY_KEY } from "./howIWorkModel";

/** Kept at module scope so the query keeps one Set for one answer: an
 *  inline `select` runs again on every render and hands back a new Set,
 *  which every memo keyed on these ids would then see as a change. */
function savedRulesPageIds(rules: SpaceRulesResponse): ReadonlySet<string> {
  return rules.saved_rules_page_ids?.length ? new Set(rules.saved_rules_page_ids) : NO_SAVED_RULES_PAGES;
}

/** The ids of the pages that keep the rules the person asked to be saved,
 *  as the daemon's ledger has them. Empty until the daemon answers, and
 *  when it cannot (an older daemon, a team member's browser): such a page
 *  is then listed as the ordinary page its file is. */
export function useSavedRulesPageIds(): ReadonlySet<string> {
  const exampleWiki = useExampleWikiActive();
  const told = useQuery({
    queryKey: SAVED_RULES_PAGES_QUERY_KEY,
    queryFn: () => getSpaceRules(null),
    select: savedRulesPageIds,
    enabled: !exampleWiki,
    staleTime: 30_000,
    retry: false,
  });
  return told.data ?? NO_SAVED_RULES_PAGES;
}
