// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { getPageLook, type PageLook } from "../../../../lib/tauri";
import { pageLooksEnabled } from "./lookFeature";

export type LookMode = "look" | "text";

export const modeKey = (pageId: string) => `wenlan-look-mode:${pageId}`;
/** Set while a look is loading; found at the next open only if the app died mid-load. */
export const loadingKey = (pageId: string) => `wenlan-look-loading:${pageId}`;
/** Names this run of the app, so a mark this run made is not mistaken for a dead one's. */
const RUN = Math.random().toString(36).slice(2);
export const loadingValue = (lookId: number) => `${lookId}:${RUN}`;

function storageGet(key: string): string | null {
  try { return window.localStorage.getItem(key); } catch { return null; }
}
function storageSet(key: string, value: string): void {
  try { window.localStorage.setItem(key, value); } catch { /* remembered only when storage works */ }
}
function storageRemove(key: string): void {
  try { window.localStorage.removeItem(key); } catch { /* nothing to clear */ }
}

export const lookQueryKey = (pageId: string) => ["page-look", pageId] as const;

/**
 * A page's look and how it is shown: the Look | Text choice remembered per
 * page, and a fallback to Text when the look cannot be shown. A failure lasts
 * for that look and that version of the page; a new look or a changed page
 * tries again, and so does choosing Look.
 */
export function usePageLook({ pageId, supported, version }: {
  pageId: string;
  supported: boolean;
  /** Changes when the page's text does. */
  version: number | string;
}) {
  const enabled = pageLooksEnabled() && supported;
  const queryClient = useQueryClient();
  const query = useQuery({
    queryKey: lookQueryKey(pageId),
    queryFn: () => getPageLook(pageId),
    enabled,
    retry: false,
    staleTime: 5_000,
    refetchInterval: 15_000,
  });
  const look: PageLook | null = enabled ? (query.data?.look ?? null) : null;

  const [chosen, setChosen] = useState<{ pageId: string; mode: LookMode } | null>(null);
  const mode: LookMode = useMemo(() => {
    if (chosen?.pageId === pageId) return chosen.mode;
    return storageGet(modeKey(pageId)) === "text" ? "text" : "look";
  }, [chosen, pageId]);

  const failureKey = look ? `${pageId}:${look.id}:${version}` : null;
  const [failedKey, setFailedKey] = useState<string | null>(null);

  // Stuck-loop guard: a look that was loading when the app last stopped is not
  // opened by itself again; the page opens on its text with the one line.
  const checked = useRef<string | null>(null);
  useEffect(() => {
    if (!look || checked.current === `${pageId}:${look.id}`) return;
    checked.current = `${pageId}:${look.id}`;
    const marker = storageGet(loadingKey(pageId));
    if (marker === null) return;
    // Only a mark from an earlier run: this run's own LookView sets one as it loads.
    if (marker === `${look.id}:${RUN}`) return;
    storageRemove(loadingKey(pageId));
    if (marker.split(":")[0] === String(look.id)) setFailedKey(`${pageId}:${look.id}:${version}`);
  }, [look, pageId, version]);

  const failed = failureKey !== null && failedKey === failureKey;
  const shown = look !== null && mode === "look" && !failed;

  const choose = useCallback((next: LookMode) => {
    setChosen({ pageId, mode: next });
    storageSet(modeKey(pageId), next);
    if (next === "look") setFailedKey(null);
  }, [pageId]);

  const markFailed = useCallback(() => {
    if (failureKey) setFailedKey(failureKey);
  }, [failureKey]);

  const refresh = useCallback(
    () => queryClient.invalidateQueries({ queryKey: lookQueryKey(pageId) }),
    [pageId, queryClient],
  );

  return { look, hasLook: look !== null, mode, shown, failed, choose, markFailed, refresh };
}
