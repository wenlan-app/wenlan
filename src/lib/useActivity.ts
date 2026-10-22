// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery, type UseQueryResult } from "@tanstack/react-query";
import { getActivity, type ActivityResponse } from "./tauri";

/** Poll cadence while there is work in flight or something is stuck. */
export const ACTIVITY_BUSY_POLL_MS = 5_000;
/** Poll cadence once everything is organized. */
export const ACTIVITY_IDLE_POLL_MS = 60_000;

export const ACTIVITY_QUERY_KEY = ["activity"] as const;

/**
 * The one read behind the toolbar Activity button, the summary popover and the Now
 * section. All three mount at once on the Activity page, so they share a
 * single query key and the daemon sees one request, not three.
 *
 * The spec asks for 5 s while the window is focused and the state is
 * Steeping or Blocked, 60 s when Up to date. Only the cadence is spelled
 * here: React Query already suspends `refetchInterval` while the window is
 * unfocused, so adding a focus listener would duplicate that and race it.
 *
 * The interval is the function form, matching SourcesView — it re-reads the
 * cached data on every tick, so the cadence follows the state without the
 * component re-subscribing.
 */
export function useActivity(): UseQueryResult<ActivityResponse> {
  return useQuery({
    queryKey: ACTIVITY_QUERY_KEY,
    queryFn: () => getActivity(),
    refetchInterval: (query) => {
      const data = query.state.data as ActivityResponse | undefined;
      // Unknown state polls at the busy rate: a first load that has not landed
      // yet is more likely to be mid-import than idle, and guessing idle would
      // leave a fresh install looking frozen for a minute.
      if (data === undefined) return ACTIVITY_BUSY_POLL_MS;
      return data.state === "up_to_date" || data.state === "off"
        ? ACTIVITY_IDLE_POLL_MS
        : ACTIVITY_BUSY_POLL_MS;
    },
  });
}
