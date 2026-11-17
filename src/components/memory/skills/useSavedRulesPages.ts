// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { getSpaceRules } from "../../../lib/tauri";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { NO_SAVED_RULES_PAGES, SAVED_RULES_PAGES_QUERY_KEY } from "./howIWorkModel";

/** The ids of the pages that keep the rules the person asked to be saved,
 *  as the daemon's ledger has them. Empty until the daemon answers, and
 *  when it cannot (an older daemon, a team member's browser): such a page
 *  is then listed as the ordinary page its file is. */
export function useSavedRulesPageIds(): ReadonlySet<string> {
  const exampleWiki = useExampleWikiActive();
  const told = useQuery({
    queryKey: SAVED_RULES_PAGES_QUERY_KEY,
    queryFn: () => getSpaceRules(null),
    select: (rules): ReadonlySet<string> => new Set(rules.saved_rules_page_ids ?? []),
    enabled: !exampleWiki,
    staleTime: 30_000,
    retry: false,
  });
  return told.data ?? NO_SAVED_RULES_PAGES;
}
