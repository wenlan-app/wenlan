// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties } from "react";
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
import { RULES_DELIVERY_QUERY_KEY, useRulesSetup, useRulesSetupReview } from "./useRulesSetup";
import "./howIWork.css";
import { rulesSetupName } from "./rulesSetupText";
import "./rulesSetup.css";

export { RULES_DELIVERY_QUERY_KEY } from "./useRulesSetup";

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
  // The brand colour in a theme where it reads on the chip; the theme's
  // text colour otherwise (the CSS falls back when a variable is unset).
  const colours = {
    ...(logo.light ? { "--logo-light": logo.light } : {}),
    ...(logo.dark ? { "--logo-dark": logo.dark } : {}),
  } as CSSProperties;
  return (
    <svg className="rules-tools-logo" aria-hidden="true" focusable="false" viewBox={logo.viewBox} fill="currentColor" data-logo={id} style={colours}>
      {logo.paths.map((path, index) => <path key={index} d={path.d} fillRule={path.evenOdd ? "evenodd" : undefined} clipRule={path.evenOdd ? "evenodd" : undefined} />)}
    </svg>
  );
}

/** The dot carries a mark as well as a colour, so the states differ for a
 *  person who cannot tell sage from amber: a check when delivered, an
 *  exclamation mark when not, a hollow ring when only connected. */
function ToolDotMark({ dot }: { readonly dot: ToolDot }) {
  return (
    <svg className="rules-tools-dot" data-dot={dot} aria-hidden="true" focusable="false" viewBox="0 0 12 12">
      {dot === "connected" ? <circle cx="6" cy="6" r="4.6" fill="none" stroke="currentColor" strokeWidth="1.5" />
        : <circle cx="6" cy="6" r="6" fill="currentColor" />}
      {dot === "delivered" ? <path className="rules-tools-dot-glyph" d="M3.4 6.2 5.2 8l3.4-3.6" fill="none" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" /> : null}
      {dot === "problem" ? <path className="rules-tools-dot-glyph" d="M6 3.2v3.4M6 8.6v.1" fill="none" strokeWidth="1.6" strokeLinecap="round" /> : null}
    </svg>
  );
}

type ShownTool = { readonly tool: ToolRulesDelivery; readonly standing: ToolStanding; readonly badge: string };

/** One tool: its logo (or a short badge), its name and a dot. Hover or
 *  keyboard focus shows a small card with the tool's name, what Wenlan can
 *  say about it, and, once delivered, how many rules it holds across all
 *  Spaces and when. A click or a tap pins the card open until the next click
 *  or a press elsewhere; Escape closes it however it opened. The pointer can
 *  move into the card, and the card stays inside the window. */
function ToolChip({ shown, review, onReview }: { readonly shown: ShownTool; readonly review: boolean; readonly onReview?: () => void }) {
  const { t, i18n } = useTranslation();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const [pinned, setPinned] = useState(false);
  const [dismissed, setDismissed] = useState(false);
  const [shift, setShift] = useState(0);
  const itemRef = useRef<HTMLSpanElement>(null);
  const surfaceRef = useRef<HTMLSpanElement>(null);
  const cardId = useId();
  const { tool, standing, badge } = shown;
  const open = !dismissed && (hovered || focused || pinned);
  const dot = toolDot(standing);
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const delivered = standing === "delivered";
  const when = delivered && tool.delivered_at ? (locale.startsWith("en") ? formatTimeAgo(tool.delivered_at) : formatTimeAgo(tool.delivered_at, locale)) : null;
  // The daemon counts the rules in the tool's file, from every Space.
  const count = delivered && tool.rule_count > 0 ? t("howIWork.tools.rulesDelivered", { count: tool.rule_count }) : null;
  const name = rulesSetupName(tool.display_name) || t("rulesSetup.thisTool");
  const detail = [count, when].filter(Boolean).join(" · ");

  useEffect(() => {
    if (!open) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      setPinned(false);
      setDismissed(true);
    };
    const unpinOutside = (event: Event) => {
      if (event.target instanceof Node && itemRef.current?.contains(event.target)) return;
      setPinned(false);
    };
    document.addEventListener("keydown", closeOnEscape, true);
    document.addEventListener("pointerdown", unpinOutside, true);
    return () => {
      document.removeEventListener("keydown", closeOnEscape, true);
      document.removeEventListener("pointerdown", unpinOutside, true);
    };
  }, [open]);

  // Keep the card inside the window: move it left by what would spill.
  useLayoutEffect(() => {
    if (!open) { setShift(0); return; }
    const rect = surfaceRef.current?.getBoundingClientRect();
    if (!rect) return;
    const left = rect.left - shift;
    const right = rect.right - shift;
    const room = window.innerWidth - 8;
    const next = right > room ? Math.max(room - right, 8 - left) : 0;
    if (Math.abs(next - shift) > 0.5) setShift(next);
  }, [open, shift]);

  return (
    <span
      className="rules-tools-item"
      data-standing={standing}
      ref={itemRef}
      onMouseEnter={() => { setHovered(true); setDismissed(false); }}
      onMouseLeave={() => { setHovered(false); setDismissed(false); }}
    >
      <button
        type="button"
        className="rules-tools-chip"
        aria-describedby={cardId}
        aria-label={review ? t("rulesSetup.chipNeedsReview", { tool: name }) : undefined}
        data-pinned={pinned ? "true" : undefined}
        onFocus={() => { setFocused(true); setDismissed(false); }}
        onBlur={() => { setFocused(false); setDismissed(false); }}
        onClick={() => { if (review && onReview) { setDismissed(true); onReview(); } else { setDismissed(false); setPinned((value) => !value); } }}
      >
        <ToolMark id={tool.id} badge={badge} />
        <span className="rules-tools-name">{name}</span>
        <ToolDotMark dot={review ? "problem" : dot} />
      </button>
      <span className="rules-tools-card" id={cardId} role="tooltip" hidden={!open} style={shift ? { transform: `translateX(${shift}px)` } : undefined}>
        <span className="rules-tools-card-surface mem-popover-surface" ref={surfaceRef}>
          <span className="rules-tools-card-head"><ToolMark id={tool.id} badge={badge} />{name}</span>
          <span className="rules-tools-card-state" data-dot={dot}><ToolDotMark dot={dot} /><span className="rules-tools-card-state-text">{t(`howIWork.tools.${standing}`)}</span></span>
          {detail ? <span className="rules-tools-card-detail">{detail}</span> : null}
        </span>
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
export function RulesToolsHeader({ delivers, setupOnly = false, reviewCount, skillUpdates, onReview }: {
  readonly delivers: boolean;
  readonly setupOnly?: boolean;
  readonly reviewCount?: number | null;
  readonly skillUpdates?: number;
  readonly onReview?: () => void;
}) {
  const { t } = useTranslation();
  const exampleWiki = useExampleWikiActive();
  const setup = useRulesSetup();
  const review = useRulesSetupReview(!!setup && reviewCount === undefined && !skillUpdates);
  const delivery = useQuery({
    queryKey: RULES_DELIVERY_QUERY_KEY,
    queryFn: getRulesDelivery,
    enabled: !exampleWiki && (!setupOnly || !!setup),
    staleTime: 30_000,
    retry: false,
  });
  if ((setupOnly && !setup) || (!setup && !delivery.isSuccess)) return null;
  const listed = (delivery.data ?? [])
    .map((tool) => {
      const own = toolStanding(tool);
      return { tool, standing: own !== null && !delivers ? "connected" as const : own };
    })
    .filter((item): item is { tool: ToolRulesDelivery; standing: ToolStanding } => item.standing !== null)
    .sort((a, b) => STANDING_ORDER.indexOf(a.standing) - STANDING_ORDER.indexOf(b.standing));
  // Only a tool with no logo shows a badge, so only those must differ.
  const unmarked = listed.filter((item) => !TOOL_LOGOS[item.tool.id]);
  const badges = new Map(toolBadges(unmarked.map((item) => rulesSetupName(item.tool.display_name) || t("rulesSetup.thisTool"))).map((badge, index) => [unmarked[index].tool.id, badge] as const));
  const shown: ShownTool[] = listed.map((item) => ({ ...item, badge: badges.get(item.tool.id) ?? "" }));
  const pendingToolIds = new Set([
    ...Object.values(setup?.review_tools ?? {}).map((tool) => tool.id),
    ...(setup?.tools ?? []).filter((tool) => tool.state === "needs_review").map((tool) => tool.id),
  ]);
  const pendingCount = reviewCount === undefined ? review.data?.items.length : reviewCount;
  let line: string | undefined;
  if (skillUpdates) {
    line = t("rulesSetup.skillUpdateCount", { count: skillUpdates });
  } else if (pendingCount) {
    line = t("rulesSetup.reviewCount", { count: pendingCount });
  } else if (pendingCount === 0 && setup?.adopted) {
    line = t(setup.in_sync ? "rulesSetup.inSync" : "rulesSetup.syncNeeded");
  }
  return (
    <section className="rules-tools" aria-label={t("howIWork.tools.label")} data-testid="rules-tools">
      {delivery.isSuccess && (shown.length === 0 ? (
        <p className="rules-tools-row">{t("howIWork.tools.none")}</p>
      ) : (
        <div className="rules-tools-row">
          <span className="rules-tools-lead">{t("howIWork.tools.usedIn")}</span>
          <ul className="rules-tools-list">
            {shown.map((item) => <li key={item.tool.id}><ToolChip shown={item} review={pendingToolIds.has(item.tool.id)} onReview={onReview} /></li>)}
          </ul>
        </div>
      ))}
      {line && <p className="rules-setup-status" role="status">{line}</p>}
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
