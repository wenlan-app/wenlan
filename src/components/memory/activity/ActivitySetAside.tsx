// SPDX-License-Identifier: AGPL-3.0-only
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listSetAsideMaterial, releaseSetAsideMaterial, type SetAsideGroup } from "../../../lib/tauri";
import "./activityDiscoveries.css";

export const SET_ASIDE_QUERY_KEY = ["steep", "setAside"] as const;

const REASON_KEYS = {
  too_short: "activitySetAside.reason.tooShort",
  sources_did_not_match: "activitySetAside.reason.sourcesDidNotMatch",
} as const;

function reasonKey(reason: string) {
  return reason in REASON_KEYS ? REASON_KEYS[reason as keyof typeof REASON_KEYS] : "activitySetAside.reason.other" as const;
}

/**
 * "Not written yet" (UX-HOME-7): the material Automatic tried to write as a
 * page and set aside, one row per group with the reason. Home stays quiet
 * about it; Automatic tries an item again when it changes, and "Try again"
 * gives a group back now. Absent when nothing is set aside.
 */
export default function ActivitySetAside() {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  // An older daemon without the route reads as none.
  const setAside = useQuery({ queryKey: SET_ASIDE_QUERY_KEY, queryFn: listSetAsideMaterial, refetchInterval: 60_000, retry: false });
  const release = useMutation({
    mutationFn: (group: SetAsideGroup) => releaseSetAsideMaterial(group.items.map((item) => item.item_id)),
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: SET_ASIDE_QUERY_KEY });
      void queryClient.invalidateQueries({ queryKey: ["newMaterial"] });
    },
  });
  const groups = (setAside.data?.groups ?? []).filter((group) => group.name.trim() && group.items.length > 0);
  if (groups.length === 0) return null;
  return (
    <section className="mem-activity-discoveries" aria-labelledby="mem-activity-set-aside-title" data-testid="activity-set-aside">
      <h2 id="mem-activity-set-aside-title" className="mem-activity-section-title">{t("activitySetAside.title")}</h2>
      <div className="mem-activity-discoveries-body">
        <p className="mem-activity-discovery-hint">{t("activitySetAside.hint")}</p>
        <section className="mem-activity-discovery-group">
          <ul>
            {groups.map((group) => (
              <li className="mem-activity-discovery-row" key={`${group.name}-${group.set_aside_at}`} data-testid="activity-set-aside-row">
                <div className="mem-activity-discovery-row-copy">
                  <h4>{group.name}</h4>
                  <p className="mem-activity-discovery-hint">{t(reasonKey(group.reason))}</p>
                </div>
                <button
                  type="button"
                  className="mem-activity-discovery-action"
                  title={t("activitySetAside.retryTitle")}
                  disabled={release.isPending}
                  onClick={() => release.mutate(group)}
                >
                  {t("activitySetAside.retry")}
                </button>
              </li>
            ))}
          </ul>
        </section>
      </div>
    </section>
  );
}
