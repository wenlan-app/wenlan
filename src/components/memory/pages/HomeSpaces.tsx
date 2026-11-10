// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { SteepMoon } from "../things/ThingSteeped";
import { ranOvernight, steepedToday } from "../../../lib/graph/lastNight";
import { listSpaces, listThings, setUiLanguage, type Space } from "../../../lib/tauri";
import { THINGS_QUERY_KEY, WORKING_ON_LISTED } from "./WorkingOn";

/** Most names the "steeped overnight" line writes out; the rest are "and more". */
const STEEPED_NAMED = 2;

/** Starred Spaces first, then the person's own order, as in the sidebar. */
export function homeSpaceOrder(spaces: readonly Space[]): Space[] {
  return spaces
    .filter((space) => !space.suggested)
    .slice()
    .sort((a, b) => Number(b.starred) - Number(a.starred) || a.sort_order - b.sort_order);
}

/**
 * Home's first section while "What you're working on" is an experiment that
 * is off (UX-HOME-1): the person's Spaces, and beside the heading the one
 * quiet line saying what last night steeped. The night still sorts
 * conversations and steeps them in the background; only the cards for that
 * work are left off this screen. The line names, never counts (UX-HOME-2).
 */
export function HomeSpaces({
  onSelectSpace,
  onBrowseSpaces,
  onReplayNight,
}: {
  readonly onSelectSpace: (spaceName: string) => void;
  readonly onBrowseSpaces?: () => void;
  readonly onReplayNight?: () => void;
}) {
  const { t, i18n } = useTranslation();
  const spaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  const things = useQuery({ queryKey: THINGS_QUERY_KEY, queryFn: listThings, refetchInterval: 60_000 });
  // The background still names what it reads in the app's language.
  const language = i18n.resolvedLanguage ?? i18n.language;
  useEffect(() => {
    if (language) void setUiLanguage(language).catch(() => {});
  }, [language]);
  const steepedAll = (things.data ?? []).filter((thing) => thing.title?.trim() && thing.dreamed_at && steepedToday(thing.dreamed_at));
  const steeped = steepedAll.slice(0, STEEPED_NAMED);
  const overnight = ranOvernight(steepedAll.map((thing) => thing.dreamed_at ?? 0));
  const ordered = homeSpaceOrder(spaces.data ?? []);
  const listed = ordered.slice(0, WORKING_ON_LISTED);
  if (listed.length === 0 && steeped.length === 0) return null;
  const label = t(overnight ? "pages.overview.steepedOvernight" : "pages.overview.steepedToday");
  return (
    <section className="wiki-working-on home-spaces" data-testid="home-spaces" aria-labelledby="home-spaces-title">
      <header className="wiki-new-pages-header wiki-working-on-header">
        <h2 id="home-spaces-title">{t("pages.overview.spacesTitle")}</h2>
        {steeped.length > 0 ? (
          <p className="wiki-working-on-steeped" data-testid="home-steeped">
            {onReplayNight ? (
              <button type="button" className="wiki-working-on-steeped-replay" title={t("pages.overview.steepedReplay")} aria-label={t("pages.overview.steepedReplay")} onClick={onReplayNight}>
                <SteepMoon />
                <span className="wiki-working-on-steeped-label">{label}</span>
              </button>
            ) : (
              <>
                <SteepMoon />
                <span className="wiki-working-on-steeped-label">{label}</span>
              </>
            )}
            <span className="wiki-working-on-steeped-names home-steeped-names">
              {steeped.map((thing) => thing.title).join(t("pages.overview.steepedSeparator"))}
              {steepedAll.length > STEEPED_NAMED ? ` ${t("pages.overview.steepedMore")}` : ""}
            </span>
          </p>
        ) : null}
        {ordered.length > listed.length && onBrowseSpaces ? (
          <button type="button" className="wiki-new-pages-all" onClick={onBrowseSpaces}>
            {t("pages.overview.spacesAll")}
          </button>
        ) : null}
      </header>
      {listed.length > 0 ? (
        <ul>
          {listed.map((space) => (
            <li key={space.id}>
              <button type="button" className="wiki-working-on-card home-space-card" onClick={() => onSelectSpace(space.name)}>
                <span className="wiki-working-on-title" title={space.name}>{space.name}</span>
                <span className="wiki-working-on-text" title={space.description ?? undefined}>{space.description ?? ""}</span>
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
