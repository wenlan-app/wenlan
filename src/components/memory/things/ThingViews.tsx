// SPDX-License-Identifier: AGPL-3.0-only
import { useState, type KeyboardEvent } from "react";
import { useTranslation } from "react-i18next";
import type { ThingDetail, ThingMemory, ThingPage } from "../../../lib/tauri";

export type ThingView = "short" | "map";

const VIEW_KEY = "wenlan-thing-view";

function readView(): ThingView {
  try {
    return window.localStorage.getItem(VIEW_KEY) === "map" ? "map" : "short";
  } catch {
    return "short";
  }
}

function writeView(view: ThingView) {
  try {
    window.localStorage.setItem(VIEW_KEY, view);
  } catch {
    // Remembering the view is a convenience; the page works without it.
  }
}

/** Oldest first, so the points read in the order they were worked out. */
function pointsOf(thing: ThingDetail): ThingMemory[] {
  return [...(thing.memories ?? [])].sort((a, b) => a.created_at - b.created_at);
}

/**
 * A thing's two views (docs/ux/spec.md, UX-VIEW-1): short text and mind map
 * show the same points; switching does not leave the page, and the last view
 * used is remembered.
 */
export function ThingViews({
  thing,
  meta,
  onOpenPage,
}: {
  readonly thing: ThingDetail;
  readonly meta: string;
  readonly onOpenPage: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const [view, setView] = useState<ThingView>(readView);
  const choose = (next: ThingView) => {
    setView(next);
    writeView(next);
  };
  const points = pointsOf(thing);
  return (
    <>
      <div className="thing-views-bar">
        <p className="thing-detail-meta">{meta}</p>
        <div className="thing-views-switch" role="group" aria-label={t("thing.viewsLabel")}>
          <button type="button" aria-pressed={view === "short"} onClick={() => choose("short")}>
            {t("thing.viewShort")}
          </button>
          <button type="button" aria-pressed={view === "map"} onClick={() => choose("map")}>
            {t("thing.viewMap")}
          </button>
        </div>
      </div>
      {view === "short" ? (
        <div className="thing-short" data-testid="thing-short">
          {thing.status_line ? <p className="thing-detail-where">{thing.status_line}</p> : null}
          {points.length > 0 ? (
            RECAP_GROUPS.map((group) => (
              <RecapGroup key={group.key} group={group} points={points.filter((point) => groupOf(point) === group.key)} />
            ))
          ) : thing.status_line ? null : (
            <p className="thing-detail-empty">{t("thing.noPoints")}</p>
          )}
        </div>
      ) : (
        <ThingMap thing={thing} points={points} onOpenPage={onOpenPage} />
      )}
    </>
  );
}

type RecapKey = "decided" | "background";

/** A recap in a fixed order (docs/ux/spec.md, UX-VIEW-6): where it stands and
 *  the next step first, then what is decided, then the background. Each point
 *  already carries its kind, so nothing is rewritten to sort it. "Still open"
 *  joins between decided and background once points can say they are open. */
type RecapLabel = "thing.recapDecided" | "thing.recapBackground";

const RECAP_GROUPS: readonly { readonly key: RecapKey; readonly label: RecapLabel }[] = [
  { key: "decided", label: "thing.recapDecided" },
  { key: "background", label: "thing.recapBackground" },
];

const RECAP_SHOWN = 3;

function groupOf(point: ThingMemory): RecapKey {
  switch (point.memory_type) {
    case "decision":
      return "decided";
    default:
      return "background";
  }
}

function RecapGroup({
  group,
  points,
}: {
  readonly group: { readonly key: RecapKey; readonly label: RecapLabel };
  readonly points: readonly ThingMemory[];
}) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(false);
  if (points.length === 0) return null;
  // Newest first within a group; the rest wait behind one line.
  const newest = [...points].sort((a, b) => b.created_at - a.created_at);
  const shown = open ? newest : newest.slice(0, RECAP_SHOWN);
  const hidden = newest.length - shown.length;
  return (
    <section className="thing-recap-group" aria-label={t(group.label)} data-recap={group.key}>
      <h3>{t(group.label)}</h3>
      <ul>
        {shown.map((point) => (
          <li key={point.source_id}>{withStop(point.title)}</li>
        ))}
      </ul>
      {hidden > 0 ? (
        <button type="button" className="thing-recap-more" onClick={() => setOpen(true)}>
          {t("thing.recapMore", { count: hidden })}
        </button>
      ) : null}
    </section>
  );
}

function withStop(text: string): string {
  const trimmed = text.trim();
  return /[.!?。！？…]$/.test(trimmed) ? trimmed : `${trimmed}.`;
}

const MAP_WIDTH = 730;
const POINT_WIDTH = 220;
const GROUP_X = 244;
const GROUP_WIDTH = 92;
const CENTER_X = 372;
const CENTER_WIDTH = 150;
const PAGE_X = 556;
const PAGE_WIDTH = MAP_WIDTH - PAGE_X;
const LINE_HEIGHT = 17;
const NODE_PAD_Y = 10;
const NODE_GAP = 10;
const GROUP_GAP = 22;

/** Split a label into at most two lines that fit a node, ending in an ellipsis
 *  when cut. CJK characters count about twice as wide as Latin ones. */
function wrap(text: string, maxUnits = 30): string[] {
  const units = (ch: string) => (/[⺀-￿]/.test(ch) ? 2 : 1);
  const lines: string[] = [];
  let line = "";
  let width = 0;
  const words = /[⺀-￿]/.test(text) ? [...text] : text.split(/(\s+)/);
  for (const word of words) {
    const w = [...word].reduce((sum, ch) => sum + units(ch), 0);
    if (width + w > maxUnits && line.trim()) {
      lines.push(line.trim());
      if (lines.length === 2) break;
      line = "";
      width = 0;
      if (/^\s+$/.test(word)) continue;
    }
    line += word;
    width += w;
  }
  if (lines.length < 2 && line.trim()) lines.push(line.trim());
  const used = lines.join(" ").replace(/\s+/g, "").length;
  if (used < text.replace(/\s+/g, "").length && lines.length > 0) {
    lines[lines.length - 1] = `${lines[lines.length - 1].replace(/[\s.,;:，。；：]*$/, "")}…`;
  }
  return lines;
}

const nodeHeight = (lines: readonly string[]) => lines.length * LINE_HEIGHT + NODE_PAD_Y * 2;

function NodeText({ x, y, lines, anchor }: { readonly x: number; readonly y: number; readonly lines: readonly string[]; readonly anchor?: "middle" }) {
  return (
    <text x={x} y={y + NODE_PAD_Y + LINE_HEIGHT / 2} dominantBaseline="middle" textAnchor={anchor}>
      {lines.map((line, index) => (
        <tspan key={index} x={x} dy={index === 0 ? 0 : LINE_HEIGHT}>{line}</tspan>
      ))}
    </text>
  );
}

const curve = (x1: number, y1: number, x2: number, y2: number) => {
  const mid = (x1 + x2) / 2;
  return `M${x1} ${y1} C${mid} ${y1} ${mid} ${y2} ${x2} ${y2}`;
};

/** The thing in the middle; on the left its points under the same three kinds
 *  as the short text (newest three each, the rest counted); on the right the
 *  pages it used, which open from the map. */
function ThingMap({
  thing,
  points,
  onOpenPage,
}: {
  readonly thing: ThingDetail;
  readonly points: readonly ThingMemory[];
  readonly onOpenPage: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  // Left side: groups, each a label with its points stacked beside it.
  let y = 0;
  const groups = RECAP_GROUPS.flatMap((group) => {
    const all = points.filter((point) => groupOf(point) === group.key).sort((a, b) => b.created_at - a.created_at);
    if (all.length === 0) return [];
    const items = all.slice(0, RECAP_SHOWN).map((point) => ({ key: point.source_id, full: point.title, lines: wrap(point.title), more: false }));
    if (all.length > RECAP_SHOWN) {
      const label = t("thing.recapMore", { count: all.length - RECAP_SHOWN });
      items.push({ key: `${group.key}-more`, full: label, lines: [label], more: true });
    }
    const top = y;
    const placed = items.map((item) => {
      const node = { ...item, y, height: nodeHeight(item.lines) };
      y += node.height + NODE_GAP;
      return node;
    });
    const bottom = y - NODE_GAP;
    y += GROUP_GAP - NODE_GAP;
    return [{ key: group.key, label: t(group.label), top, middle: (top + bottom) / 2, items: placed }];
  });
  const leftHeight = Math.max(0, y - GROUP_GAP);
  let py = 0;
  const pages = thing.pages.map((page: ThingPage) => {
    const lines = wrap(page.title, 24);
    const node = { key: page.id, full: page.title, lines, y: py, height: nodeHeight(lines) };
    py += node.height + NODE_GAP;
    return node;
  });
  const rightHeight = Math.max(0, py - NODE_GAP);
  const centerLines = wrap(thing.title ?? t("thing.untitled"), 18);
  const centerHeight = nodeHeight(centerLines) + 4;
  const height = Math.max(leftHeight, rightHeight, centerHeight) + 24;
  const leftTop = (height - leftHeight) / 2;
  const rightTop = (height - rightHeight) / 2;
  const cy = height / 2;
  const open = (pageId: string) => (event: KeyboardEvent<SVGGElement>) => {
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      onOpenPage(pageId);
    }
  };
  return (
    <div className="thing-map" data-testid="thing-map">
      <svg viewBox={`0 0 ${MAP_WIDTH} ${height}`} width="100%" role="group" aria-label={t("thing.viewMap")}>
        <g className="thing-map-links" fill="none">
          {groups.map((group) => (
            <g key={group.key}>
              <path d={curve(GROUP_X + GROUP_WIDTH, leftTop + group.middle, CENTER_X, cy)} />
              {group.items.map((item) => (
                <path key={item.key} d={curve(POINT_WIDTH, leftTop + item.y + item.height / 2, GROUP_X, leftTop + group.middle)} />
              ))}
            </g>
          ))}
          {pages.map((page) => (
            <path key={page.key} d={curve(CENTER_X + CENTER_WIDTH, cy, PAGE_X, rightTop + page.y + page.height / 2)} />
          ))}
        </g>
        <g className="thing-map-center">
          <rect x={CENTER_X} y={cy - centerHeight / 2} width={CENTER_WIDTH} height={centerHeight} rx={14} />
          <NodeText x={CENTER_X + CENTER_WIDTH / 2} y={cy - centerHeight / 2 + 2} lines={centerLines} anchor="middle" />
        </g>
        {groups.map((group) => (
          <g key={group.key} role="group" aria-label={group.label}>
            <g className="thing-map-group">
              <rect x={GROUP_X} y={leftTop + group.middle - 15} width={GROUP_WIDTH} height={30} rx={15} />
              <text x={GROUP_X + GROUP_WIDTH / 2} y={leftTop + group.middle} dominantBaseline="middle" textAnchor="middle">{group.label}</text>
            </g>
            {group.items.map((item) => (
              <g key={item.key} className={item.more ? "thing-map-point thing-map-more" : "thing-map-point"}>
                <title>{item.full}</title>
                <rect x={0} y={leftTop + item.y} width={POINT_WIDTH} height={item.height} rx={10} />
                <NodeText x={12} y={leftTop + item.y} lines={item.lines} />
              </g>
            ))}
          </g>
        ))}
        {pages.map((page) => (
          <g
            key={page.key}
            className="thing-map-page"
            role="button"
            tabIndex={0}
            aria-label={page.full}
            onClick={() => onOpenPage(page.key)}
            onKeyDown={open(page.key)}
          >
            <title>{page.full}</title>
            <rect x={PAGE_X} y={rightTop + page.y} width={PAGE_WIDTH} height={page.height} rx={10} />
            <NodeText x={PAGE_X + 12} y={rightTop + page.y} lines={page.lines} />
          </g>
        ))}
      </svg>
    </div>
  );
}
