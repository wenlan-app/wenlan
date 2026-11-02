// SPDX-License-Identifier: AGPL-3.0-only
import { useQueries, useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { hasToolMark, ToolMark } from "../assets/toolMarks";
import { agentDisplayName, getThing, listThings, listWorkingOn, type ThingDetail, type ThingSummary, type WorkingOnItem } from "../../../lib/tauri";

export const WORKING_ON_QUERY_KEY = ["working-on"] as const;
export const THINGS_QUERY_KEY = ["things"] as const;

/** How many lines of work home shows; the rest live in their Spaces. */
export const WORKING_ON_LISTED = 6;

/** "Today", "yesterday", "3 days ago" in the interface language: when the
 *  work was added, a day and never a counter of things. */
export function addedWhen(language: string, addedAtSeconds: number, now = Date.now()): string {
  const days = Math.max(0, Math.floor((now - addedAtSeconds * 1000) / 86_400_000));
  const format = new Intl.RelativeTimeFormat(language, { numeric: "auto" });
  return days < 7 ? format.format(-days, "day") : format.format(-Math.floor(days / 7), "week");
}

/**
 * The one line on where a thing stands. The newest handoff says it best; until
 * a conversation has handed off, the page it wrote, or else the tool it was
 * talked through in, says what there is to pick up.
 */
export function whereItStands(t: TFunction, thing: ThingSummary, detail?: ThingDetail): string | null {
  if (thing.status_line?.trim()) return thing.status_line;
  const wrote = detail?.pages.filter((page) => page.via === "wrote").sort((a, b) => b.at - a.at)[0];
  if (wrote) return t("pages.overview.workingOnWrote", { title: wrote.title });
  const session = detail?.sessions.slice().sort((a, b) => b.last_write_at - a.last_write_at)[0];
  const tool = agentDisplayName(session?.tool);
  return tool ? t("pages.overview.workingOnTalkedIn", { tool }) : null;
}

/**
 * Home's first section: what you're working on (docs/ux/spec.md, UX-HOME-1).
 * Each thing gets one line saying where it stands, and opening it goes to
 * that thing. Until things have their own record (an older daemon, or none
 * sorted yet), each active item of a Space Brief stands for one thing and
 * opens its Space. Nothing shows when there is no work, and the section never
 * counts anything (UX-HOME-2).
 */
export function WorkingOn({
  onSelectSpace,
  onOpenThing,
}: {
  readonly onSelectSpace: (spaceName: string) => void;
  readonly onOpenThing?: (thingId: string) => void;
}) {
  const { t, i18n } = useTranslation();
  const things = useQuery({
    queryKey: THINGS_QUERY_KEY,
    queryFn: listThings,
    refetchInterval: 60_000,
    enabled: !!onOpenThing,
  });
  const thingItems: ThingSummary[] = onOpenThing ? (things.data ?? []).slice(0, WORKING_ON_LISTED) : [];
  // Each card names the pages its work wrote and the tool it happened in.
  const details = useQueries({
    queries: thingItems.map((thing) => ({
      queryKey: [...THINGS_QUERY_KEY, thing.id],
      queryFn: () => getThing(thing.id),
      staleTime: 60_000,
    })),
  });
  const useBrief = !onOpenThing || (things.isFetched && thingItems.length === 0);
  const brief = useQuery({
    queryKey: WORKING_ON_QUERY_KEY,
    queryFn: listWorkingOn,
    refetchInterval: 60_000,
    enabled: useBrief,
  });
  const briefItems: WorkingOnItem[] = useBrief ? (brief.data ?? []).slice(0, WORKING_ON_LISTED) : [];
  if (thingItems.length === 0 && briefItems.length === 0) return null;
  return (
    <section className="wiki-working-on" data-testid="wiki-working-on" aria-labelledby="wiki-working-on-title">
      <h2 id="wiki-working-on-title">{t("pages.overview.workingOnTitle")}</h2>
      <ul>
        {thingItems.map((thing, index) => {
          const detail = details[index]?.data;
          const line = whereItStands(t, thing, detail);
          // A tool with a mark shows the mark; others keep their name.
          const slugs = [...new Set((detail?.sessions ?? []).map((session) => session.tool).filter((tool): tool is string => !!tool))];
          const marked = slugs.filter(hasToolMark);
          const named = [...new Set(slugs.filter((tool) => !hasToolMark(tool)).map((tool) => agentDisplayName(tool)).filter(Boolean))];
          const where = [...named, ...thing.spaces].join(" · ");
          return (
            <li key={thing.id}>
              <button type="button" className="wiki-working-on-card" onClick={() => onOpenThing?.(thing.id)}>
                <span className="wiki-working-on-title">{thing.title ?? t("thing.untitled")}</span>
                <span className="wiki-working-on-text">{line ?? ""}</span>
                <span className="wiki-working-on-foot">
                  <span className="wiki-working-on-meta">
                    {marked.length > 0 ? (
                      <span className="wiki-working-on-tools">
                        {marked.map((tool) => <ToolMark key={tool} tool={tool} />)}
                      </span>
                    ) : null}
                    {where ? <span className="wiki-working-on-where">{where}</span> : null}
                  </span>
                  <span className="wiki-working-on-when">{addedWhen(i18n.language, thing.last_activity_at)}</span>
                </span>
              </button>
            </li>
          );
        })}
        {briefItems.map((item) => (
          <li key={`${item.space}\u0000${item.id}`}>
            <button type="button" className="wiki-working-on-card" onClick={() => onSelectSpace(item.space)}>
              <span className="wiki-working-on-title">{item.text}</span>
              <span className="wiki-working-on-foot">
                <span className="wiki-working-on-meta">{item.space}</span>
                <span className="wiki-working-on-when">{addedWhen(i18n.language, item.added_at)}</span>
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
