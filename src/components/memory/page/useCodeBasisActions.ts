// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import {
  confirmPageCodeBasis,
  listCodeBasisPending,
  removePageCodeBasis,
  type CodeBasisLocator,
  type CodeBasisPendingEntry,
  type CodeBasisRef,
} from "../../../lib/tauri";

export const CODE_BASIS_PENDING_QUERY_KEY = ["code-basis-pending"] as const;

/** `${pageId}\0${repo_root}\0${path}\0${line_start}\0${line_end}`: one row of one note. */
export const codeBasisKeyOf = (pageId: string, row: CodeBasisLocator) =>
  [pageId, row.repo_root, row.path, row.line_start, row.line_end].join("\u0000");

/** The rows that need a look: changed lines, or code that cannot be found. */
export function codeBasisNeedingAttention(rows: readonly CodeBasisRef[] | undefined): CodeBasisRef[] {
  return (rows ?? []).filter((row) => row.state === "pending" || row.state === "unavailable");
}

/**
 * Notes whose code changed or went missing. The daemon re-reads the files
 * behind every code basis to answer, so this is asked for once and kept for a
 * while rather than on every render.
 */
export function useCodeBasisPending(enabled = true) {
  const query = useQuery({
    queryKey: CODE_BASIS_PENDING_QUERY_KEY,
    queryFn: () => listCodeBasisPending(null),
    enabled,
    staleTime: 30_000,
    retry: false,
  });
  const entries: CodeBasisPendingEntry[] = (query.data ?? []).filter(
    (entry) => codeBasisNeedingAttention(entry.pending).length > 0,
  );
  return { entries, isLoading: query.isLoading, isError: query.isError };
}

export type CodeBasisConfirmAnswer = "confirmed" | "conflict" | "error";

/**
 * The two things a person can do about a code basis without editing: say the
 * change has no impact, or stop relying on the lines. Both refresh everything
 * that shows code bases. A conflict (the file changed again while they looked)
 * refetches and is reported through `hasChangedAgain`.
 */
export function useCodeBasisActions() {
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState<ReadonlySet<string>>(new Set());
  const [changedAgain, setChangedAgain] = useState<ReadonlySet<string>>(new Set());
  // What went wrong for a row: the confirm or the stop, so the message names the right one.
  const [failed, setFailed] = useState<ReadonlyMap<string, "confirm" | "stop">>(new Map());

  const refresh = useCallback((pageId: string) => {
    void queryClient.invalidateQueries({ queryKey: ["page", pageId] });
    void queryClient.invalidateQueries({ queryKey: CODE_BASIS_PENDING_QUERY_KEY });
  }, [queryClient]);

  const track = useCallback(async <T,>(
    key: string,
    kind: "confirm" | "stop",
    run: () => Promise<T>,
    fallback: T,
  ): Promise<T> => {
    setBusy((current) => new Set(current).add(key));
    setFailed((current) => {
      const next = new Map(current);
      next.delete(key);
      return next;
    });
    try {
      return await run();
    } catch {
      setFailed((current) => new Map(current).set(key, kind));
      return fallback;
    } finally {
      setBusy((current) => {
        const next = new Set(current);
        next.delete(key);
        return next;
      });
    }
  }, []);

  const confirm = useCallback(async (pageId: string, row: CodeBasisLocator): Promise<CodeBasisConfirmAnswer> => {
    const key = codeBasisKeyOf(pageId, row);
    const answer = await track<CodeBasisConfirmAnswer>(key, "confirm", async () => {
      const result = await confirmPageCodeBasis(pageId, row);
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

  const stop = useCallback(async (pageId: string, row: CodeBasisLocator): Promise<boolean> => {
    const key = codeBasisKeyOf(pageId, row);
    const done = await track<boolean>(key, "stop", async () => {
      await removePageCodeBasis(pageId, row);
      return true;
    }, false);
    if (done) refresh(pageId);
    return done;
  }, [refresh, track]);

  return {
    confirm,
    stop,
    refresh,
    isBusy: (pageId: string, row: CodeBasisLocator) => busy.has(codeBasisKeyOf(pageId, row)),
    hasChangedAgain: (pageId: string, row: CodeBasisLocator) => changedAgain.has(codeBasisKeyOf(pageId, row)),
    /** "confirm" or "stop" when the last attempt on the row failed, else null. */
    failureOf: (pageId: string, row: CodeBasisLocator) => failed.get(codeBasisKeyOf(pageId, row)) ?? null,
  };
}
