// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  getRulesDelivery,
  listSpaceRulesSuggestions,
  skillsFromLibraryEnabled,
  type ToolRulesDelivery,
} from "../../../lib/tauri";
import { useExampleWikiActive } from "../../../lib/exampleWiki/mode";
import { HOW_I_WORK_QUERY_KEYS } from "./howIWorkModel";
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

/** Heads a rules page: which connected AI tools the rules are used in, and
 *  whether Wenlan delivered them. Nothing shows until the daemon answers; an
 *  older daemon that cannot answer shows nothing. */
export function RulesToolsHeader() {
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
  const groups = STANDING_ORDER
    .map((standing) => ({ standing, tools: delivery.data.filter((tool) => toolStanding(tool) === standing) }))
    .filter((group) => group.tools.length > 0);
  return (
    <section className="rules-tools" aria-label={t("howIWork.tools.label")} data-testid="rules-tools">
      {groups.length === 0 ? (
        <p className="rules-tools-row">{t("howIWork.tools.none")}</p>
      ) : groups.map((group, index) => (
        <p className="rules-tools-row" data-standing={group.standing} key={group.standing}>
          <span className="rules-tools-lead" aria-hidden={index > 0 ? "true" : undefined} data-quiet={index > 0 ? "true" : undefined}>{t("howIWork.tools.usedIn")}</span>
          {group.tools.map((tool) => <span className="rules-tools-chip" key={tool.id}>{tool.display_name}</span>)}
          <span className="rules-tools-standing">{t(`howIWork.tools.${group.standing}`)}</span>
        </p>
      ))}
    </section>
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
