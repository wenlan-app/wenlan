// SPDX-License-Identifier: AGPL-3.0-only
import { createContext, useContext, useId, useLayoutEffect } from "react";

/** The docked note list: 240px plus its 8px leading margin in wikiWorkspace.css. */
export const WIKI_LIST_DOCKED_WIDTH = 248;
/** The wiki-main container width at which the list stacks into the Notes dropdown (wikiWorkspace.css). */
export const WIKI_LIST_STACK_MAX_WIDTH = 900;

/** A column beside the reading lane: the note list or Info. */
export type WorkspaceDemandKind = "list" | "info";

export interface WorkspaceDemand {
  readonly report: (key: string, kind: WorkspaceDemandKind, width: number) => void;
}

/** Docked side columns report their width so the shell can decide whether the labelled rail still fits. */
export const WorkspaceDemandContext = createContext<WorkspaceDemand | null>(null);

/** Report this column's docked width; zero when it is closed. */
export function useReportWorkspaceDemand(kind: WorkspaceDemandKind, width: number) {
  const demand = useContext(WorkspaceDemandContext);
  const key = useId();
  useLayoutEffect(() => {
    demand?.report(key, kind, width);
  }, [demand, key, kind, width]);
  useLayoutEffect(() => () => demand?.report(key, kind, 0), [demand, key, kind]);
}

/** The widest reported column of each kind; two note groups share one Info lane. */
export function workspaceDemandWidths(reports: Readonly<Record<string, { kind: WorkspaceDemandKind; width: number }>>) {
  let list = 0;
  let info = 0;
  for (const { kind, width } of Object.values(reports)) {
    if (kind === "list") list = Math.max(list, width);
    else info = Math.max(info, width);
  }
  return { list, info };
}
