// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listThings, listWorkingOn, type ThingSummary, type WorkingOnItem } from "../../../lib/tauri";

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
        {thingItems.map((thing) => (
          <li key={thing.id}>
            <button type="button" className="wiki-working-on-card" onClick={() => onOpenThing?.(thing.id)}>
              <span className="wiki-working-on-title">{thing.title ?? t("thing.untitled")}</span>
              {thing.status_line ? <span className="wiki-working-on-text">{thing.status_line}</span> : null}
              <span className="wiki-working-on-meta">
                {[...thing.spaces, addedWhen(i18n.language, thing.last_activity_at)].join(" · ")}
              </span>
            </button>
          </li>
        ))}
        {briefItems.map((item) => (
          <li key={`${item.space}\u0000${item.id}`}>
            <button type="button" className="wiki-working-on-card" onClick={() => onSelectSpace(item.space)}>
              <span className="wiki-working-on-text">{item.text}</span>
              <span className="wiki-working-on-meta">
                {item.space} · {addedWhen(i18n.language, item.added_at)}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
