// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  getRulesDelivery,
  getSpaceFolders,
  getSpaceRules,
  listSpaceRulesSuggestions,
  removeSavedRule,
  skillsFromLibraryEnabled,
  type Page,
  type SkillLine,
  type SpaceRulesResponse,
  type ToolRulesDelivery,
} from "../../../lib/tauri";
import { pageSection, spaceLayout } from "../pages/pageSections";
import { SPACE_FOLDERS_QUERY_KEY } from "../pages/SpaceFoldersPrompt";
import { useScope } from "../team/scope";
import { HowIWorkLines } from "./HowIWorkLines";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { HOW_I_WORK_QUERY_KEYS, pageGivesGeneralRules, rulesFromPage, savedRuleAlreadyRemoved, savedRuleRemoveFailure, toolBadges } from "./howIWorkModel";
import { TOOL_LOGOS } from "./toolLogos";
import { formatTimeAgo } from "../../../lib/dateFormat";
import { useSavedRulesPageIds } from "./useSavedRulesPages";
import { SpaceRulesAsk } from "./SpaceRulesAsk";
import "./howIWork.css";

export const RULES_DELIVERY_QUERY_KEY = ["rules-delivery"] as const;

/** What the header may say about a connected tool. It only ever says
 *  "connected" and "delivered" (or not): what Wenlan did, never what the AI
 *  does with the rules (UX-SKILL-8). */
export type ToolStanding = "delivered" | "connected" | "pending" | "overLimit" | "blocked" | "notDelivered";
const STANDING_ORDER: readonly ToolStanding[] = ["delivered", "connected", "pending", "overLimit", "blocked", "notDelivered"];

export function toolStanding(tool: ToolRulesDelivery): ToolStanding | null {
  if (!tool.connected || tool.state === "not_connected") return null;
  // A tool that asks Wenlan for the rules has no file they were written
  // to, so it is never shown as delivered.
  if (tool.method === "mcp") return "connected";
  switch (tool.state) {
    case "delivered":
    case "covered_by_another_file":
      return "delivered";
    case "on_request":
      return "connected";
    case "out_of_date":
      return "pending";
    case "over_limit":
      return "overLimit";
    case "blocked":
      return "blocked";
    default:
      return "notDelivered";
  }
}

/** The rules the daemon lists for the Space a page is in (every Space's
 *  rules when it is in none), read as the person: who wrote each one, and
 *  which are used in every Space. Asked again when the page changes. */
export function usePageRules(page: Pick<Page, "id" | "version">, spaceName: string | null): SpaceRulesResponse | undefined {
  const exampleWiki = useExampleWikiActive();
  const rules = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.rules(spaceName ?? "", page.id, page.version),
    queryFn: () => getSpaceRules(spaceName),
    enabled: !exampleWiki,
    retry: false,
  });
  return rules.data;
}

/** The dot a tool's chip carries: filled when Wenlan delivered the rules,
 *  hollow when the tool is only connected, amber when they were not
 *  delivered (not yet, too long, a file Wenlan cannot change). */
export type ToolDot = "delivered" | "connected" | "problem";
export function toolDot(standing: ToolStanding): ToolDot {
  return standing === "delivered" || standing === "connected" ? standing : "problem";
}

function ToolMark({ id, badge }: { readonly id: string; readonly badge: string }) {
  const logo = TOOL_LOGOS[id];
  if (!logo) return <span className="rules-tools-badge" aria-hidden="true">{badge}</span>;
  return (
    <svg className="rules-tools-logo" aria-hidden="true" focusable="false" viewBox={logo.viewBox} fill="currentColor" data-logo={id}>
      {logo.paths.map((path, index) => <path key={index} d={path.d} fillRule={path.evenOdd ? "evenodd" : undefined} clipRule={path.evenOdd ? "evenodd" : undefined} />)}
    </svg>
  );
}

type ShownTool = { readonly tool: ToolRulesDelivery; readonly standing: ToolStanding; readonly badge: string };

/** One tool: its logo (or a short badge), its name and a dot. Hover or
 *  keyboard focus opens a small card with the tool's name, what Wenlan can
 *  say about it, and, once delivered, how many rules and when. */
function ToolChip({ shown }: { readonly shown: ShownTool }) {
  const { t, i18n } = useTranslation();
  const [open, setOpen] = useState(false);
  const cardId = useId();
  const { tool, standing, badge } = shown;
  const dot = toolDot(standing);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const delivered = standing === "delivered";
  const when = delivered && tool.delivered_at ? (locale.startsWith("en") ? formatTimeAgo(tool.delivered_at) : formatTimeAgo(tool.delivered_at, locale)) : null;
  const count = delivered && tool.rule_count > 0 ? t("howIWork.tools.rulesDelivered", { count: tool.rule_count }) : null;
  const detail = [count, when].filter(Boolean).join(" · ");
  return (
    <span className="rules-tools-item" data-standing={standing} onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
      <button
        type="button"
        className="rules-tools-chip"
        aria-describedby={cardId}
        onFocus={() => setOpen(true)}
        onBlur={() => setOpen(false)}
        onClick={() => setOpen((value) => !value)}
        onKeyDown={(event) => { if (event.key === "Escape" && open) { event.stopPropagation(); setOpen(false); } }}
      >
        <ToolMark id={tool.id} badge={badge} />
        <span className="rules-tools-name">{tool.display_name}</span>
        <span className="rules-tools-dot" data-dot={dot} aria-hidden="true" />
      </button>
      <span className="rules-tools-card mem-popover-surface" id={cardId} role="tooltip" hidden={!open}>
        <span className="rules-tools-card-head"><ToolMark id={tool.id} badge={badge} />{tool.display_name}</span>
        <span className="rules-tools-card-state" data-dot={dot}><span className="rules-tools-dot" data-dot={dot} aria-hidden="true" /><span className="rules-tools-card-state-text">{t(`howIWork.tools.${standing}`)}</span></span>
        {detail ? <span className="rules-tools-card-detail">{detail}</span> : null}
      </span>
    </span>
  );
}

/** Heads a rules page: which connected AI tools the rules are used in, and
 *  whether Wenlan delivered them, in one row of small chips. Only rules used
 *  in every Space are written into the tools' files, so `delivers` says
 *  whether this page gives any; without one, each connected tool reads
 *  "connected" and nothing more. Nothing shows until the daemon answers; an
 *  older daemon that cannot answer shows nothing. */
export function RulesToolsHeader({ delivers }: { readonly delivers: boolean }) {
  const { t } = useTranslation();
  const exampleWiki = useExampleWikiActive();
  const delivery = useQuery({
    queryKey: RULES_DELIVERY_QUERY_KEY,
    queryFn: getRulesDelivery,
    enabled: !exampleWiki,
    staleTime: 30_000,
    retry: false,
  });
  if (!delivery.isSuccess) return null;
  const listed = delivery.data
    .map((tool) => {
      const own = toolStanding(tool);
      return { tool, standing: own !== null && !delivers ? "connected" as const : own };
    })
    .filter((item): item is { tool: ToolRulesDelivery; standing: ToolStanding } => item.standing !== null)
    .sort((a, b) => STANDING_ORDER.indexOf(a.standing) - STANDING_ORDER.indexOf(b.standing));
  // Only a tool with no logo shows a badge, so only those must differ.
  const unmarked = listed.filter((item) => !TOOL_LOGOS[item.tool.id]);
  const badges = new Map(toolBadges(unmarked.map((item) => item.tool.display_name)).map((badge, index) => [unmarked[index].tool.id, badge] as const));
  const shown: ShownTool[] = listed.map((item) => ({ ...item, badge: badges.get(item.tool.id) ?? "" }));
  return (
    <section className="rules-tools" aria-label={t("howIWork.tools.label")} data-testid="rules-tools">
      {shown.length === 0 ? (
        <p className="rules-tools-row">{t("howIWork.tools.none")}</p>
      ) : (
        <div className="rules-tools-row">
          <span className="rules-tools-lead">{t("howIWork.tools.usedIn")}</span>
          <ul className="rules-tools-list">
            {shown.map((item) => <li key={item.tool.id}><ToolChip shown={item} /></li>)}
          </ul>
        </div>
      )}
    </section>
  );
}

/** Heads a page the person keeps in a Rules folder, and a page that keeps
 *  the rules they asked to be saved (the daemon says which pages those are),
 *  and shows nothing on any other page: the same header as a "How I work"
 *  page, then each rule the daemon reads from this page with who wrote it,
 *  then what is still waiting for an answer. */
export function RulesFolderPageHead({ page, onOpenPage }: { readonly page: Page; readonly onOpenPage: (pageId: string) => void }) {
  const savedRulesPages = useSavedRulesPageIds();
  // No folder named Rules on the page's path, and not a saved-rules page:
  // nothing more to ask the daemon.
  return pageSection(page, undefined, savedRulesPages) === "rules" ? <RulesFolderGate page={page} onOpenPage={onOpenPage} savedRulesPages={savedRulesPages} /> : null;
}

function RulesFolderGate({ page, onOpenPage, savedRulesPages }: { readonly page: Page; readonly onOpenPage: (pageId: string) => void; readonly savedRulesPages: ReadonlySet<string> }) {
  const scope = useScope();
  const spaceFolders = useQuery({ queryKey: SPACE_FOLDERS_QUERY_KEY, queryFn: () => getSpaceFolders(), retry: false, staleTime: Infinity });
  // The folder may be a Space's own, which is no rules folder. The page's
  // own Space folder is enough to tell.
  if (!spaceFolders.isFetched) return null;
  const section = pageSection(page, spaceLayout([page], spaceFolders.data === true, scope.teamSpaces), savedRulesPages);
  return section === "rules" ? <RulesFolderRules page={page} onOpenPage={onOpenPage} /> : null;
}

function RulesFolderRules({ page, onOpenPage }: { readonly page: Page; readonly onOpenPage: (pageId: string) => void }) {
  const { t } = useTranslation();
  const space = page.space?.trim();
  const spaceName = space && space !== "uncategorized" ? space : null;
  const rules = usePageRules(page, spaceName);
  const lines = rulesFromPage(rules, page.id);
  const queryClient = useQueryClient();
  const [alreadyRemoved, setAlreadyRemoved] = useState(false);
  // Whatever the press came to, the rules are read again: a rule taken out
  // somewhere else must not stay listed here with a Remove that cannot work.
  const readAgain = async () => {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["page", page.id] }),
      queryClient.invalidateQueries({ queryKey: ["space-rules"] }),
    ]);
    void queryClient.invalidateQueries({ queryKey: ["pages"] });
    void queryClient.invalidateQueries({ queryKey: ["page-revisions", page.id] });
    void queryClient.invalidateQueries({ queryKey: RULES_DELIVERY_QUERY_KEY });
  };
  // One press takes a saved rule off its page; the page's history keeps it.
  const removeSaved = async (line: SkillLine): Promise<string | null> => {
    setAlreadyRemoved(false);
    try {
      const outcome = await removeSavedRule(line.id);
      await readAgain();
      return outcome.status === "waiting" ? t("howIWork.saved.waiting") : null;
    } catch (error) {
      await readAgain().catch(() => undefined);
      if (savedRuleAlreadyRemoved(error)) {
        // Its row goes with the fresh list, so this is said for the page.
        setAlreadyRemoved(true);
        return null;
      }
      return savedRuleRemoveFailure(error, t);
    }
  };
  return (
    <>
      <RulesToolsHeader delivers={pageGivesGeneralRules(rules, page, [], spaceName === null)} />
      {lines.length > 0 && (
        <section className="rules-page-lines" aria-label={t("howIWork.rulesOnPage")} data-testid="rules-page-lines">
          <h2 className="rules-page-lines-title">{t("howIWork.rulesOnPage")}</h2>
          <HowIWorkLines lines={lines} onOpenPage={onOpenPage} ownPageId={page.id} showEdited={false} onRemoveSaved={removeSaved} />
        </section>
      )}
      {alreadyRemoved ? <p className="how-i-work-line-remove-note rules-page-removed-note" role="status" data-testid="rules-already-removed">{t("howIWork.saved.alreadyRemoved")}</p> : null}
      <RulesPagePending spaceName={page.space} onOpenPage={onOpenPage} />
    </>
  );
}

/** On a rules page: the rules the AI drew from this Space's pages that
 *  nobody answered yet, in their own box with Accept and No. They do not
 *  count until accepted (UX-SKILL-1). */
export function RulesPagePending({ spaceName, onOpenPage }: { readonly spaceName: string | null | undefined; readonly onOpenPage: (pageId: string) => void }) {
  const exampleWiki = useExampleWikiActive();
  const enabled = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.enabled,
    queryFn: skillsFromLibraryEnabled,
    enabled: !exampleWiki && Boolean(spaceName),
    staleTime: 30_000,
    retry: false,
  });
  const suggestions = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.spaceSuggestions,
    queryFn: listSpaceRulesSuggestions,
    enabled: enabled.data === true,
    retry: false,
  });
  const pending = suggestions.data?.find((item) => item.space_name === spaceName);
  if (!pending) return null;
  return <SpaceRulesAsk space={{ id: pending.space_id, name: pending.space_name }} onOpenPage={onOpenPage} variant="pending" />;
}
