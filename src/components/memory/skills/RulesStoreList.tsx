// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { getPersonalRulesOverview, type PersonalRulesOverview } from "../../../lib/tauri";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { useScope } from "../team/scope";
import { toolDisplayName } from "./RulesToolsHeader";
import { RULES_OVERVIEW_QUERY_KEY } from "./useRulesSetup";
import "./rulesSetup.css";


export interface RuleGroup { readonly heading: string | null; readonly body: string; readonly rules: number }

/** Splits rule text at its `##` headings. Generated comments and the file's
 *  own `#` title are not rules, so they are left out. */
export function ruleGroups(text: string): RuleGroup[] {
  const lines = text.replace(/\r\n?/g, "\n").replace(/<!--[\s\S]*?(?:-->|$)/g, "").split("\n");
  const groups: { heading: string | null; lines: string[]; rules: number }[] = [{ heading: null, lines: [], rules: 0 }];
  let fence: string | null = null;
  for (const line of lines) {
    const current = groups[groups.length - 1];
    // Inside a fenced code block every line is text: no headings, no rules.
    const marker = /^\s*(`{3,}|~{3,})/.exec(line)?.[1];
    if (fence) {
      if (marker && marker[0] === fence[0] && marker.length >= fence.length) fence = null;
      current.lines.push(line);
      continue;
    }
    if (marker) { fence = marker; current.lines.push(line); continue; }
    if (/^#\s/.test(line)) continue;
    const heading = /^##\s+(.*)$/.exec(line);
    if (heading) { groups.push({ heading: heading[1].trim(), lines: [], rules: 0 }); continue; }
    current.lines.push(line);
    if (/^\s*[-*]\s+\S/.test(line)) current.rules += 1;
  }
  return groups
    .map(({ heading, lines: body, rules }) => ({ heading, body: body.join("\n").trim(), rules }))
    .filter((group) => group.body.length > 0);
}

/** Read-only view of the rules every tool gets and each tool's own lines.
 *  Changing a rule stays a plan the person applies in To review. */
export function useRulesOverview(enabled = true) {
  const example = useExampleWikiActive();
  const { team } = useScope();
  return useQuery<PersonalRulesOverview>({
    queryKey: RULES_OVERVIEW_QUERY_KEY,
    queryFn: getPersonalRulesOverview,
    staleTime: 30_000,
    retry: false,
    // Personal rules live on this computer: the Review fixture, the example
    // wiki and a team's library have none to show.
    enabled: enabled && !__WENLAN_REVIEW__ && !example && !team,
  });
}

/** How many rules the shared store hands out, counting each tool's own lines. */
export function storeRuleCount(overview: PersonalRulesOverview | undefined): number {
  if (!overview) return 0;
  return [overview.shared, ...overview.tools.map((tool) => tool.text)]
    .flatMap(ruleGroups).reduce((sum, group) => sum + group.rules, 0);
}

/** True once the overview has loaded and holds at least one readable rule. */
export function overviewHasRules(overview: PersonalRulesOverview | undefined): boolean {
  if (!overview) return false;
  return ruleGroups(overview.shared).length > 0 || overview.tools.some((tool) => ruleGroups(tool.text).length > 0);
}

export function RulesStoreList() {
  const { t } = useTranslation();
  const overview = useRulesOverview();
  if (overview.isError) return <p className="section-list-state" role="alert">{t("rulesSetup.store.loadFailed")}</p>;
  if (!overview.isSuccess) return null;
  const shared = ruleGroups(overview.data.shared);
  const tools = overview.data.tools
    .map((tool) => ({ name: toolDisplayName(tool.tool), groups: ruleGroups(tool.text) }))
    .filter((tool) => tool.groups.length > 0);
  if (shared.length + tools.length === 0) return null;
  return (
    <section className="rules-store" aria-labelledby="rules-store-title">
      <h2 className="section-list-label" id="rules-store-title">{t("rulesSetup.store.title")}</h2>
      <p className="rules-store-hint">{t("rulesSetup.store.hint")}</p>
      {shared.map((group, index) => <RuleGroupView key={`shared-${index}`} group={group} label={group.heading ?? t("rulesSetup.store.general")} />)}
      {tools.map((tool) => (
        <RuleGroupView key={tool.name} label={t("rulesSetup.store.onlyFor", { tool: tool.name })}
          group={{ heading: null, body: tool.groups.map((group) => group.heading ? `### ${group.heading}\n\n${group.body}` : group.body).join("\n\n"), rules: tool.groups.reduce((sum, group) => sum + group.rules, 0) }} />
      ))}
    </section>
  );
}

function RuleGroupView({ group, label }: { readonly group: RuleGroup; readonly label: string }) {
  const { t } = useTranslation();
  return (
    <details className="rules-store-group">
      <summary>
        <span className="rules-store-group-title">{label}</span>
        {group.rules > 0 && <span className="rules-store-group-count">{t("rulesSetup.store.count", { count: group.rules })}</span>}
      </summary>
      <div className="rules-store-body">
        {/* Read-only text: links show where they point but never navigate the app. */}
        <ReactMarkdown remarkPlugins={[remarkGfm]} components={{ a: ({ children, href }) => <span className="rules-store-link" title={href}>{children}</span> }}>
          {group.body}
        </ReactMarkdown>
      </div>
    </details>
  );
}
