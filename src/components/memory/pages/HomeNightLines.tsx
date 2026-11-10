// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { SteepMoon } from "../things/ThingSteeped";
import { ranOvernight, steepedToday } from "../../../lib/graph/lastNight";
import { listThings } from "../../../lib/tauri";
import { THINGS_QUERY_KEY } from "./WorkingOn";

/** Most names the steep line writes out; the rest are "and more". */
const STEEPED_NAMED = 2;

/**
 * Home's first lines (UX-HOME-1): what last night steeped, across every Space
 * and the Wiki, so it sits above them rather than beside one heading. It
 * names, never counts (UX-HOME-2), and is absent on a night that did nothing.
 * Replay is a visible word, not a hidden moon. Other quiet lines about the
 * background belong in this same block, one per line.
 */
export function HomeNightLines({ onReplayNight }: { readonly onReplayNight?: () => void }) {
  const { t } = useTranslation();
  const things = useQuery({ queryKey: THINGS_QUERY_KEY, queryFn: listThings, refetchInterval: 60_000 });
  const steepedAll = (things.data ?? []).filter((thing) => thing.title?.trim() && thing.dreamed_at && steepedToday(thing.dreamed_at));
  if (steepedAll.length === 0) return null;
  const steeped = steepedAll.slice(0, STEEPED_NAMED);
  const overnight = ranOvernight(steepedAll.map((thing) => thing.dreamed_at ?? 0));
  return (
    <div className="home-night" data-testid="home-night">
      <p className="home-night-line" data-testid="home-steeped">
        <SteepMoon />
        <span className="home-night-label">{t(overnight ? "pages.overview.steepedOvernight" : "pages.overview.steepedToday")}</span>
        <span className="home-night-names">
          {steeped.map((thing) => thing.title).join(t("pages.overview.steepedSeparator"))}
          {steepedAll.length > STEEPED_NAMED ? ` ${t("pages.overview.steepedMore")}` : ""}
        </span>
        {onReplayNight ? (
          <button type="button" className="home-night-action" title={t("pages.overview.steepedReplay")} onClick={onReplayNight}>
            {t("pages.overview.steepedReplayShort")}
          </button>
        ) : null}
      </p>
    </div>
  );
}
