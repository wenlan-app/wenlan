// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  confirmPageBasis,
  listBasisPending,
  unmarkPageBasis,
  type BasisPendingEntry,
} from "../../../lib/tauri";

export const BASIS_PENDING_QUERY_KEY = ["basis-pending"] as const;

/** The review destination for "notes whose source note changed". */
export const BASIS_UPDATES_REVIEW_ID = "basis-updates";

const keyOf = (pageId: string, basisPageId: string) => `${pageId}\u0000${basisPageId}`;

/**
 * Notes whose source note has an update waiting. Only the notes that changed
 * count: an archived, deleted or moved source has nothing to confirm.
 */
export function useBasisPending(enabled = true) {
  const query = useQuery({
    queryKey: BASIS_PENDING_QUERY_KEY,
    queryFn: () => listBasisPending(null),
    enabled,
    staleTime: 30_000,
    retry: false,
  });
  const entries: BasisPendingEntry[] = (query.data ?? []).filter(
    (entry) => entry.state === "pending" && entry.citers.length > 0,
  );
  return { entries, isLoading: query.isLoading, isError: query.isError };
}

export type BasisConfirmAnswer = "confirmed" | "conflict" | "error";

/**
 * The two things a person can do about a basis without editing: say it has no
 * impact, or stop relying on it. Both refresh everything that shows bases.
 * A conflict (the basis moved on while they looked) refetches and is reported
 * through `changedAgain` so the caller can ask them to look once more.
 */
export function useBasisActions() {
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const [changedAgain, setChangedAgain] = useState<ReadonlySet<string>>(new Set());
  const [failed, setFailed] = useState<ReadonlySet<string>>(new Set());

  const refresh = useCallback((pageId: string) => {
    void queryClient.invalidateQueries({ queryKey: ["page", pageId] });
    void queryClient.invalidateQueries({ queryKey: ["page-basis-diff", pageId] });
    void queryClient.invalidateQueries({ queryKey: BASIS_PENDING_QUERY_KEY });
  }, [queryClient]);

  const track = useCallback(async <T,>(key: string, run: () => Promise<T>, fallback: T): Promise<T> => {
    setBusy((current) => new Set(current).add(key));
    setFailed((current) => {
      const next = new Set(current);
      next.delete(key);
      return next;
    });
    try {
      return await run();
    } catch {
      setFailed((current) => new Set(current).add(key));
      return fallback;
    } finally {
      setBusy((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
    }
  }, []);

  const confirm = useCallback(async (
    pageId: string,
    basisPageId: string,
    version: number,
  ): Promise<BasisConfirmAnswer> => {
    const key = keyOf(pageId, basisPageId);
    const answer = await track<BasisConfirmAnswer>(key, async () => {
      const result = await confirmPageBasis(pageId, basisPageId, version);
      return result.outcome === "confirmed" ? "confirmed" : "conflict";
    }, "error");
    setChangedAgain((current) => {
      const next = new Set(current);
      if (answer === "conflict") next.add(key);
      else next.delete(key);
      return next;
    });
    if (answer !== "error") refresh(pageId);
    return answer;
  }, [refresh, track]);

  const stop = useCallback(async (pageId: string, basisPageId: string): Promise<boolean> => {
    const key = keyOf(pageId, basisPageId);
    const done = await track<boolean>(key, async () => {
      await unmarkPageBasis(pageId, basisPageId);
      return true;
    }, false);
    if (done) {
      refresh(pageId);
      // The source note's own count of notes relying on it changed too.
      void queryClient.invalidateQueries({ queryKey: ["page", basisPageId] });
    }
    return done;
  }, [queryClient, refresh, track]);

  /** Mark a conflict seen from outside (a save that carried `confirm_basis`). */
  const noteChangedAgain = useCallback((pageId: string, basisPageId: string) => {
    setChangedAgain((current) => new Set(current).add(keyOf(pageId, basisPageId)));
    refresh(pageId);
  }, [refresh]);

  return {
    confirm,
    stop,
    noteChangedAgain,
    refresh,
    isBusy: (pageId: string, basisPageId: string) => busy.has(keyOf(pageId, basisPageId)),
    hasChangedAgain: (pageId: string, basisPageId: string) => changedAgain.has(keyOf(pageId, basisPageId)),
    hasFailed: (pageId: string, basisPageId: string) => failed.has(keyOf(pageId, basisPageId)),
  };
}
