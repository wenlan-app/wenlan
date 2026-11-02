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
            <p className="thing-short-points">
              {points.map((point) => (
                <span key={point.source_id} className="thing-short-point">{withStop(point.title)} </span>
              ))}
            </p>
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

function withStop(text: string): string {
  const trimmed = text.trim();
  return /[.!?。！？…]$/.test(trimmed) ? trimmed : `${trimmed}.`;
}

const MAP_WIDTH = 730;
const NODE_WIDTH = 230;
const LINE_HEIGHT = 17;
const NODE_PAD_Y = 10;
const NODE_GAP = 12;
const CENTER_WIDTH = 170;

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

interface MapNode {
  readonly key: string;
  readonly lines: string[];
  readonly full: string;
  readonly y: number;
  readonly height: number;
}

function layout(labels: readonly { key: string; text: string }[]): { nodes: MapNode[]; height: number } {
  let y = 0;
  const nodes = labels.map(({ key, text }) => {
    const lines = wrap(text);
    const height = lines.length * LINE_HEIGHT + NODE_PAD_Y * 2;
    const node = { key, lines, full: text, y, height };
    y += height + NODE_GAP;
    return node;
  });
  return { nodes, height: Math.max(0, y - NODE_GAP) };
}

/** The thing in the middle, its points on the left and the pages it used on
 *  the right. Pages open from the map. */
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
  const left = layout(points.map((point) => ({ key: point.source_id, text: point.title })));
  const right = layout(thing.pages.map((page: ThingPage) => ({ key: page.id, text: page.title })));
  const centerLines = wrap(thing.title ?? t("thing.untitled"), 22);
  const centerHeight = centerLines.length * LINE_HEIGHT + NODE_PAD_Y * 2 + 4;
  const height = Math.max(left.height, right.height, centerHeight) + 24;
  const leftTop = (height - left.height) / 2;
  const rightTop = (height - right.height) / 2;
  const cx = MAP_WIDTH / 2;
  const cy = height / 2;
  const centerLeft = cx - CENTER_WIDTH / 2;
  const centerRight = cx + CENTER_WIDTH / 2;
  const rightX = MAP_WIDTH - NODE_WIDTH;
  const curve = (x1: number, y1: number, x2: number, y2: number) => {
    const mid = (x1 + x2) / 2;
    return `M${x1} ${y1} C${mid} ${y1} ${mid} ${y2} ${x2} ${y2}`;
  };
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
          {left.nodes.map((node) => (
            <path key={node.key} d={curve(NODE_WIDTH, leftTop + node.y + node.height / 2, centerLeft, cy)} />
          ))}
          {right.nodes.map((node) => (
            <path key={node.key} d={curve(centerRight, cy, rightX, rightTop + node.y + node.height / 2)} />
          ))}
        </g>
        <g className="thing-map-center">
          <rect x={centerLeft} y={cy - centerHeight / 2} width={CENTER_WIDTH} height={centerHeight} rx={14} />
          <text x={cx} y={cy - ((centerLines.length - 1) * LINE_HEIGHT) / 2} textAnchor="middle" dominantBaseline="middle">
            {centerLines.map((line, index) => (
              <tspan key={index} x={cx} dy={index === 0 ? 0 : LINE_HEIGHT}>{line}</tspan>
            ))}
          </text>
        </g>
        {left.nodes.map((node) => (
          <g key={node.key} className="thing-map-point">
            <title>{node.full}</title>
            <rect x={0} y={leftTop + node.y} width={NODE_WIDTH} height={node.height} rx={10} />
            <text x={12} y={leftTop + node.y + NODE_PAD_Y + LINE_HEIGHT / 2} dominantBaseline="middle">
              {node.lines.map((line, index) => (
                <tspan key={index} x={12} dy={index === 0 ? 0 : LINE_HEIGHT}>{line}</tspan>
              ))}
            </text>
          </g>
        ))}
        {right.nodes.map((node) => (
          <g
            key={node.key}
            className="thing-map-page"
            role="button"
            tabIndex={0}
            aria-label={node.full}
            onClick={() => onOpenPage(node.key)}
            onKeyDown={open(node.key)}
          >
            <title>{node.full}</title>
            <rect x={rightX} y={rightTop + node.y} width={NODE_WIDTH} height={node.height} rx={10} />
            <text x={rightX + 12} y={rightTop + node.y + NODE_PAD_Y + LINE_HEIGHT / 2} dominantBaseline="middle">
              {node.lines.map((line, index) => (
                <tspan key={index} x={rightX + 12} dy={index === 0 ? 0 : LINE_HEIGHT}>{line}</tspan>
              ))}
            </text>
          </g>
        ))}
      </svg>
    </div>
  );
}
