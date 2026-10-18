// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useMemo, useState } from "react";
import { expandedSidebarWidth, ICONS_SIDEBAR_WIDTH, type SidebarPreferences } from "./navigationPreferences";
import { WIKI_LIST_STACK_MAX_WIDTH, workspaceDemandWidths, type WorkspaceDemand, type WorkspaceDemandKind } from "./workspaceDemand";

/** The narrowest note the labelled rail may leave beside the list and Info. */
export const READING_LANE_MIN_WIDTH = 600;
const AUTO_TRANSITION_MS = 200;

type DemandReports = Readonly<Record<string, { kind: WorkspaceDemandKind; width: number }>>;

function windowWidth() {
  return typeof window === "undefined" ? 0 : window.innerWidth;
}

/**
 * Fold the labelled rail to icons while the rail, the docked list, Info and a
 * readable note do not fit side by side. The saved preference is untouched, so
 * the labels return as soon as the space does.
 */
export function useAutoCompactSidebar(preferences: SidebarPreferences, enabled: boolean) {
  const [reports, setReports] = useState<DemandReports>({});
  const demand = useMemo<WorkspaceDemand>(() => ({
    report: (key, kind, width) => setReports((current) => {
      const previous = current[key];
      if (width <= 0) {
        if (!previous) return current;
        const { [key]: _removed, ...rest } = current;
        return rest;
      }
      if (previous?.kind === kind && previous.width === width) return current;
      return { ...current, [key]: { kind, width } };
    }),
  }), []);
  const [width, setWidth] = useState(windowWidth);
  useEffect(() => {
    const update = () => setWidth(windowWidth());
    update();
    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, []);
  const { list, info } = workspaceDemandWidths(reports);
  // A manual expand holds until the window or Info changes.
  const situation = `${width}:${info > 0}`;
  const [keptExpanded, setKeptExpanded] = useState<string | null>(null);
  useEffect(() => {
    setKeptExpanded((current) => current === situation ? current : null);
  }, [situation]);
  const labelsWidth = expandedSidebarWidth(preferences);
  // The wiki list docks only while the area beside the rail is wide enough,
  // so its share depends on the rail, not on this decision's own outcome.
  const listDocked = (railWidth: number) => list > 0 && width - railWidth > WIKI_LIST_STACK_MAX_WIDTH;
  const fits = labelsWidth + (listDocked(labelsWidth) ? list : 0) + info + READING_LANE_MIN_WIDTH <= width;
  // Folding must not pull a stacked list back in beside the note.
  const foldDocksList = !listDocked(labelsWidth) && listDocked(ICONS_SIDEBAR_WIDTH);
  const compact = enabled && preferences.visible && preferences.mode === "labels" && !fits && !foldDocksList && keptExpanded !== situation;
  const [settled, setSettled] = useState(compact);
  useEffect(() => {
    if (settled === compact) return;
    const timer = window.setTimeout(() => setSettled(compact), AUTO_TRANSITION_MS);
    return () => window.clearTimeout(timer);
  }, [compact, settled]);
  // A manual expand is immediate, like the rail toggle elsewhere.
  const keepExpanded = useCallback(() => {
    setKeptExpanded(situation);
    setSettled(false);
  }, [situation]);
  return { compact, demand, keepExpanded, transitioning: compact !== settled };
}
