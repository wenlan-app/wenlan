// SPDX-License-Identifier: AGPL-3.0-only
import { Fragment, useEffect, useRef, useState } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import { hasToolMark, ToolMark } from "../assets/toolMarks";
import { SteepMoon } from "../things/ThingSteeped";
import { steepedToday } from "../things/ThingViews";
import { agentDisplayName, getThing, hideThing, listThings, listWorkingOn, setUiLanguage, type ThingDetail, type ThingSummary, type WorkingOnItem } from "../../../lib/tauri";

export const WORKING_ON_QUERY_KEY = ["working-on"] as const;
export const THINGS_QUERY_KEY = ["things"] as const;

const RETRY_SOON = { retry: 5, retryDelay: (attempt: number) => Math.min(2_000 * 2 ** attempt, 15_000) } as const;

/** How many lines of work home shows before "See all". */
export const WORKING_ON_LISTED = 6;
/** Most things the "steeped overnight" line names. */
const STEEPED_NAMED = 3;
/** Steeping before this local hour happened overnight. */
const NIGHT_ENDS = 7;
/** How long a card lights up when its name is pressed. */
const GLOW_MS = 1_600;

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
 * talked through in, says what there is to pick up. A tool whose mark already
 * shows on the card is not named again.
 */
export function whereItStands(t: TFunction, thing: ThingSummary, detail?: ThingDetail): string | null {
  if (thing.status_line?.trim()) return thing.status_line;
  const wrote = detail?.pages.filter((page) => page.via === "wrote").sort((a, b) => b.at - a.at)[0];
  if (wrote) return t("pages.overview.workingOnWrote", { title: wrote.title });
  const session = detail?.sessions.slice().sort((a, b) => b.last_write_at - a.last_write_at)[0];
  if (hasToolMark(session?.tool)) return null;
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
  // A read that fails (the daemon restarting after an update) would look the
  // same as having no work, so try again within seconds, not at the next
  // minute's refresh.
  const things = useQuery({
    queryKey: THINGS_QUERY_KEY,
    queryFn: listThings,
    refetchInterval: 60_000,
    enabled: !!onOpenThing,
    ...RETRY_SOON,
  });
  // Wenlan names the things it reads from conversations in the app's
  // language, so the daemon is told which one that is.
  const language = i18n.resolvedLanguage ?? i18n.language;
  const readsThings = !!onOpenThing;
  useEffect(() => {
    if (readsThings && language) void setUiLanguage(language).catch(() => {});
  }, [readsThings, language]);
  const [showAll, setShowAll] = useState(false);
  const queryClient = useQueryClient();
  // "Don't show" takes a finished thing off home at once rather than waiting
  // out its quiet week; nothing is deleted and its next conversation brings
  // it back (UX-HOME-5).
  const hide = useMutation({
    mutationFn: (id: string) => hideThing(id),
    onMutate: (id: string) => {
      queryClient.setQueryData<ThingSummary[]>(THINGS_QUERY_KEY, (list) =>
        list?.map((thing) => (thing.id === id ? { ...thing, quiet: true } : thing)));
    },
    onSettled: () => queryClient.invalidateQueries({ queryKey: THINGS_QUERY_KEY, exact: true }),
  });
  // A thing with nothing new for a week leaves this list; search still finds
  // it, and its next conversation brings it back (UX-HOME-5). There is no
  // cap: when more are current, "See all" opens the rest in place.
  const current: ThingSummary[] = onOpenThing ? (things.data ?? []).filter((thing) => !thing.quiet) : [];
  const more = current.length > WORKING_ON_LISTED;
  const thingItems = showAll ? current : current.slice(0, WORKING_ON_LISTED);
  // What Wenlan steeped from today's things into long-term memory (UX-CHAT-5),
  // named in one quiet line beside the heading so the night's work is seen.
  // It names things, never counts them, and is gone tomorrow.
  const steeped = onOpenThing
    ? (things.data ?? []).filter((thing) => thing.title && thing.dreamed_at && steepedToday(thing.dreamed_at)).slice(0, STEEPED_NAMED)
    : [];
  const overnight = steeped.every((thing) => new Date((thing.dreamed_at ?? 0) * 1000).getHours() < NIGHT_ENDS);
  const [glowing, setGlowing] = useState<string | null>(null);
  const glowTimer = useRef<number | undefined>(undefined);
  useEffect(() => () => window.clearTimeout(glowTimer.current), []);
  // A name opens nothing new: its card lights up for a moment where it is.
  // A thing gone quiet has no card, so its name opens the thing.
  const showSteeped = (id: string) => {
    const at = current.findIndex((thing) => thing.id === id);
    if (at < 0) {
      onOpenThing?.(id);
      return;
    }
    if (at >= WORKING_ON_LISTED) setShowAll(true);
    setGlowing(id);
    window.clearTimeout(glowTimer.current);
    glowTimer.current = window.setTimeout(() => setGlowing(null), GLOW_MS);
    window.requestAnimationFrame(() =>
      document.querySelector(`[data-thing-id="${CSS.escape(id)}"]`)?.scrollIntoView?.({ behavior: "smooth", block: "nearest", inline: "nearest" }));
  };
  // Each card names the pages its work wrote and the tool it happened in.
  const details = useQueries({
    queries: thingItems.map((thing) => ({
      queryKey: [...THINGS_QUERY_KEY, thing.id],
      queryFn: () => getThing(thing.id),
      staleTime: 60_000,
    })),
  });
  // A failed things read is not "no things": wait for it instead of falling
  // back to the Space Briefs. Things that have all gone quiet are not "no
  // things" either: the Briefs they came from are just as old.
  const useBrief = !onOpenThing || (things.isSuccess && (things.data ?? []).length === 0);
  const brief = useQuery({
    queryKey: WORKING_ON_QUERY_KEY,
    queryFn: listWorkingOn,
    refetchInterval: 60_000,
    enabled: useBrief,
    ...RETRY_SOON,
  });
  const briefItems: WorkingOnItem[] = useBrief ? (brief.data ?? []).slice(0, WORKING_ON_LISTED) : [];
  if (thingItems.length === 0 && briefItems.length === 0) return null;
  return (
    <section className="wiki-working-on" data-testid="wiki-working-on" data-expanded={showAll ? "true" : undefined} aria-labelledby="wiki-working-on-title">
      <header className="wiki-new-pages-header wiki-working-on-header">
        <h2 id="wiki-working-on-title">{t("pages.overview.workingOnTitle")}</h2>
        {steeped.length > 0 ? (
          <p className="wiki-working-on-steeped" data-testid="wiki-working-on-steeped">
            <SteepMoon />
            <span className="wiki-working-on-steeped-label">{t(overnight ? "pages.overview.steepedOvernight" : "pages.overview.steepedToday")}</span>
            <span className="wiki-working-on-steeped-names">
              {steeped.map((thing, index) => (
                <Fragment key={thing.id}>
                  {index > 0 ? t("pages.overview.steepedSeparator") : null}
                  <button type="button" onClick={() => showSteeped(thing.id)}>{thing.title}</button>
                </Fragment>
              ))}
            </span>
          </p>
        ) : null}
        {more ? (
          <button type="button" className="wiki-new-pages-all" aria-expanded={showAll} onClick={() => setShowAll((shown) => !shown)}>
            {showAll ? t("pages.overview.workingOnFewer") : t("pages.overview.workingOnAll")}
          </button>
        ) : null}
      </header>
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
            <li key={thing.id} data-thing-id={thing.id}>
              <button type="button" className="wiki-working-on-card" data-glow={glowing === thing.id ? "true" : undefined} onClick={() => onOpenThing?.(thing.id)}>
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
              <button
                type="button"
                className="wiki-working-on-hide"
                aria-label={t("pages.overview.workingOnHide", { title: thing.title ?? t("thing.untitled") })}
                title={t("pages.overview.workingOnHideHint")}
                onClick={() => hide.mutate(thing.id)}
              >
                <svg viewBox="0 0 16 16" width="12" height="12" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" /></svg>
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
