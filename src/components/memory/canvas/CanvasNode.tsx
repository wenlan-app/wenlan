// SPDX-License-Identifier: AGPL-3.0-only
import { memo, useEffect, useRef } from "react";
import { Handle, Position, type Node, type NodeProps } from "@xyflow/react";
import { useTranslation } from "react-i18next";
import {
  mixOver,
  type GraphPalette,
  type GraphSlot,
} from "../../../lib/graph/palette";
import type { PageMapRefKind, PageMapStatus } from "../../../lib/tauri";

// Page-map ref kinds are not entity types, so they get their own mapping onto
// the validated 5-slot Atlas palette rather than going through
// slotForEntityType. Same inks, same theme reactivity, no new colors.
const SLOT_BY_REF_KIND: Record<PageMapRefKind, GraphSlot> = {
  page: "project",
  memory: "neutral",
  entity: "concept",
  section: "tool",
  idea: "concept",
  outline: "neutral",
};

export interface CanvasNodeData extends Record<string, unknown> {
  label: string;
  refKind: PageMapRefKind;
  status: PageMapStatus;
  dangling: boolean;
  isRoot: boolean;
  /** Rings out from the page: 0 is the page, 1 its sections, 2+ their insides. */
  depth: number;
  /**
   * Direction away from the page at the middle, radians, screen axes. The grow
   * dot rides this side of the box.
   */
  outward: number;
  readOnly: boolean;
  palette: GraphPalette;
  width: number;
  height: number;
  /**
   * Show the name field instead of the label. True for the box being renamed
   * and for the unsaved draft box.
   */
  editing: boolean;
  /** Prompt for the name field; the draft box has no label to show yet. */
  placeholder?: string;
  nameInputLabel?: string;
  onNameChange?: (label: string) => void;
  onOpen: () => void;
  /** Drawn as a quiet text link rather than a box ("show the rest"). */
  link?: boolean;
  /** The whole line behind a shortened label, shown on hover. */
  hint?: string;
  /** Set only on a heading that folds: whether its items all show. */
  unfolded?: boolean;
  onToggleFold?: () => void;
  /** A link to another page: drawn with a page glyph and a toggle that opens it. */
  door?: boolean;
  /** Grown from another page: tinted so it reads as that page's, not this one's. */
  band?: boolean;
  /** Replaces the fold toggle's label (a door says "open", not "show items"). */
  toggleLabel?: string;
  /** The ink of the page heading this line sits under; replaces the kind ink. */
  branchColor?: string;
  /** Drawn as text on a thin line of the branch ink instead of a box. */
  underline?: boolean;
  /** Outside the branch in focus: kept visible but quiet. */
  dimmed?: boolean;
  onAccept: () => void;
  onDismiss: () => void;
  onCommit: (label: string) => void | Promise<void>;
  onCancel: () => void;
}

export type CanvasNodeType = Node<CanvasNodeData, "pageMapNode">;

// Both handles sit at the box center with zero opacity: a mind map reads as
// center-to-center spokes, and edge-anchored handles would kink every spoke
// on a radial layout.
const CENTER_HANDLE = {
  left: "50%",
  top: "50%",
  transform: "translate(-50%, -50%)",
  opacity: 0,
  pointerEvents: "none",
} as const;

// Which side of the box the grow dot sits on, from the direction the box faces.
// Quadrants clockwise from three o'clock, because screen y grows downward.
const SIDES = [Position.Right, Position.Bottom, Position.Left, Position.Top];

/**
 * A radial map grows outward, so the dot you grab belongs on the side facing
 * away from the page at the middle. Pinned to the bottom it pointed back at the
 * parent for every box above the center, sat between a box and its own children
 * for every box below it, and told everyone the map was a downward tree.
 */
function growSide(outward: number): Position {
  const quadrant = Math.round(outward / (Math.PI / 2));
  return SIDES[((quadrant % 4) + 4) % 4];
}

function CanvasNode({
  data,
  selected,
  isConnectable,
}: NodeProps<CanvasNodeType>) {
  const { t } = useTranslation();
  const {
    label,
    refKind,
    status,
    dangling,
    isRoot,
    depth,
    outward,
    readOnly,
    palette,
    width,
    height,
    editing,
    placeholder,
    nameInputLabel,
  } = data;

  const slotColor = data.branchColor ?? palette[SLOT_BY_REF_KIND[refKind]];
  const underline = !!data.underline && !data.link && !editing;
  const suggested = status === "suggested";
  const showControls = suggested && !isRoot && !readOnly && !editing;

  // A mind map is read by its hierarchy, and until this ladder existed a leaf
  // was indistinguishable from a section: same fill, same border, same weight.
  // Three rungs — the page, its sections, what is inside them — spend no new
  // color, only weight, so the slot ink still means what it means everywhere
  // else in the app.
  const branch = depth <= 1;
  const fill = data.door || data.band
    ? "var(--mem-linked-band)"
    : mixOver(slotColor, palette.surface, isRoot ? 0.22 : branch ? 0.1 : 0.035);

  return (
    <div
      style={{
        width,
        height,
        display: "flex",
        alignItems: "center",
        gap: 6,
        boxSizing: "border-box",
        padding: "0 10px",
        borderRadius: underline ? 0 : "var(--mem-r-10)",
        border: data.link || underline
          ? "1px solid transparent"
          : data.door || data.band
            ? `1px ${data.band ? "dashed" : "solid"} var(--mem-linked-band-edge)`
            : `${isRoot ? 1.5 : 1}px ${suggested ? "dashed" : "solid"} ${
                branch ? slotColor : mixOver(slotColor, palette.surface, 0.5)
              }`,
        backgroundColor: data.link || underline ? "transparent" : fill,
        opacity: data.dimmed ? 0.28 : dangling ? 0.45 : suggested ? 0.75 : 1,
        transition: "opacity 160ms ease",
        // Selection has to be visible or the keyboard shortcuts have no
        // referent — "Delete removes the selected box" is meaningless if
        // nothing on screen says which box that is.
        // Past the first ring a line is text on a thin stroke of its branch
        // ink, thinner the deeper it sits, so depth reads without more boxes.
        boxShadow:
          [
            underline
              ? `inset 0 -${depth <= 2 ? 1.5 : 1}px 0 ${mixOver(slotColor, palette.surface, 0.7)}`
              : null,
            selected ? `0 0 0 2px ${slotColor}` : null,
          ]
            .filter(Boolean)
            .join(", ") || undefined,
      }}
    >
      <Handle
        type="target"
        position={Position.Top}
        isConnectable={false}
        style={CENTER_HANDLE}
      />
      {data.door && !editing && (
        <svg width="12" height="14" viewBox="0 0 12 14" fill="none" stroke="currentColor" strokeWidth="1.2" aria-hidden="true" style={{ flexShrink: 0, color: palette.labelMuted }}>
          <path d="M1 1h6l4 4v8H1z" />
          <path d="M7 1v4h4" />
        </svg>
      )}
      {editing ? (
        <NodeNameInput
          value={label}
          placeholder={placeholder}
          ariaLabel={nameInputLabel}
          onNameChange={data.onNameChange}
          color={palette.label}
          onCommit={data.onCommit}
          onCancel={data.onCancel}
        />
      ) : (
        <button
          type="button"
          onClick={data.onOpen}
          title={dangling ? t("pageCanvas.dangling") : (data.hint ?? label)}
          aria-label={
            dangling ? t("pageCanvas.danglingNode", { label }) : label
          }
          style={{
            flex: 1,
            minWidth: 0,
            textAlign: "left",
            background: "none",
            border: "none",
            // The box's selection ring is this canvas's focus indicator, and
            // Tab is bound to "add box inside" so nothing ever lands here by
            // keyboard. Left on, the browser drew a second ring tight around
            // the label the moment a shortcut was pressed after a click —
            // two rings in two colors, the inner one looking like a text field
            // the name could be typed into.
            outline: "none",
            padding: 0,
            cursor: "pointer",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            color: dangling || data.link ? palette.labelMuted : palette.label,
            textDecoration: data.link ? "underline" : undefined,
            textUnderlineOffset: data.link ? 3 : undefined,
            fontFamily: "var(--mem-font-body)",
            fontSize: 12,
            fontWeight: isRoot || data.door ? 600 : branch ? 500 : 400,
          }}
        >
          {label}
        </button>
      )}
      {data.unfolded !== undefined && data.onToggleFold && !editing && (
        <button
          type="button"
          onClick={(e) => {
            e.stopPropagation();
            data.onToggleFold?.();
          }}
          aria-expanded={data.unfolded}
          aria-label={data.toggleLabel ?? t(data.unfolded ? "pageCanvas.foldItems" : "pageCanvas.unfoldItems", { label })}
          title={data.toggleLabel ?? t(data.unfolded ? "pageCanvas.foldItems" : "pageCanvas.unfoldItems", { label })}
          style={controlStyle(palette.labelMuted)}
        >
          <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" aria-hidden="true">
            {data.unfolded ? <polyline points="15 18 9 12 15 6" /> : <polyline points="9 18 15 12 9 6" />}
          </svg>
        </button>
      )}
      {showControls && (
        <>
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              data.onAccept();
            }}
            aria-label={t("pageCanvas.accept")}
            title={t("pageCanvas.accept")}
            style={controlStyle(palette.label)}
          >
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" aria-hidden="true">
              <polyline points="20 6 9 17 4 12" />
            </svg>
          </button>
          <button
            type="button"
            onClick={(e) => {
              e.stopPropagation();
              data.onDismiss();
            }}
            aria-label={t("pageCanvas.dismiss")}
            title={t("pageCanvas.dismiss")}
            style={controlStyle(palette.labelMuted)}
          >
            <svg width="11" height="11" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" aria-hidden="true">
              <path d="M18 6L6 18M6 6l12 12" />
            </svg>
          </button>
        </>
      )}
      <Handle
        id="anchor"
        type="source"
        position={Position.Bottom}
        isConnectable={false}
        style={CENTER_HANDLE}
      />
      {/* The dot people actually grab, kept separate from the centered anchor
          above so edges keep routing center-to-center across the spine. */}
      <Handle
        id="grow"
        type="source"
        position={growSide(outward)}
        className="page-canvas-grow"
        isConnectable={isConnectable}
      />
    </div>
  );
}

/**
 * The name field, mounted only while a box is being named.
 *
 * Blur only moves focus. A box is created or renamed after an explicit Enter
 * or pointer confirmation, so opening a sidebar cannot commit half-finished
 * text. The one-shot guard keeps Enter and the confirm control from racing.
 */
function NodeNameInput({
  value,
  placeholder,
  ariaLabel,
  onNameChange,
  color,
  onCommit,
  onCancel,
}: {
  value: string;
  placeholder?: string;
  ariaLabel?: string;
  onNameChange?: (label: string) => void;
  color: string;
  onCommit: (label: string) => void | Promise<void>;
  onCancel: () => void;
}) {
  const { t } = useTranslation();
  const cancelled = useRef(false);
  const committed = useRef(false);
  const ref = useRef<HTMLInputElement>(null);
  const confirm = () => {
    if (committed.current) return;
    committed.current = true;
    try {
      const pending = onCommit(ref.current?.value ?? value);
      if (pending) {
        void pending.catch(() => {
          committed.current = false;
        });
      }
    } catch {
      committed.current = false;
    }
  };
  const cancel = () => {
    if (committed.current || cancelled.current) return;
    cancelled.current = true;
    onCancel();
  };

  // React Flow mounts a freshly added node with visibility:hidden while it
  // measures it, and a field that isn't visible cannot take focus — so React's
  // autoFocus lands on nothing and every keystroke goes to the canvas instead.
  // Ask again each frame until it sticks.
  useEffect(() => {
    let frame = 0;
    let tries = 0;
    const grab = () => {
      const el = ref.current;
      if (!el || document.activeElement === el) return;
      el.focus();
      el.select();
      if (document.activeElement !== el && ++tries < 30) {
        frame = requestAnimationFrame(grab);
      }
    };
    grab();
    return () => cancelAnimationFrame(frame);
  }, []);

  return (
    <div className="page-canvas-node-name">
      <input
        ref={ref}
        // Without these, React Flow drags the box and pans the canvas while the
        // user is trying to type into it.
        className="nodrag nopan nowheel"
        defaultValue={value}
        placeholder={placeholder}
        aria-label={ariaLabel ?? t("pageCanvas.nameSectionLabel")}
        onChange={(event) => {
          committed.current = false;
          onNameChange?.(event.currentTarget.value);
        }}
        onKeyDown={(e) => {
          // Canvas shortcuts must not fire while a name is being typed.
          e.stopPropagation();
          if (e.key === "Enter") {
            e.preventDefault();
            confirm();
          } else if (e.key === "Escape") {
            e.preventDefault();
            cancel();
          }
        }}
        // Moving focus to the sidebar or another canvas tool is not a commit.
        style={{
          flex: 1,
          minWidth: 0,
          background: "none",
          border: "none",
          outline: "none",
          padding: 0,
          color,
          fontFamily: "var(--mem-font-body)",
          fontSize: 12,
        }}
      />
      <button type="button" className="page-canvas-name-confirm nodrag nopan"
        aria-label={t("pageCanvas.confirmName")} title={t("pageCanvas.confirmName")}
        onClick={(event) => { event.stopPropagation(); confirm(); }}>
        <span aria-hidden="true">✓</span>
      </button>
      <button type="button" className="page-canvas-name-cancel nodrag nopan"
        aria-label={t("pageCanvas.cancelName")} title={t("pageCanvas.cancelName")}
        onClick={(event) => { event.stopPropagation(); cancel(); }}>
        <span aria-hidden="true">×</span>
      </button>
    </div>
  );
}

function controlStyle(color: string): React.CSSProperties {
  return {
    display: "flex",
    alignItems: "center",
    justifyContent: "center",
    flexShrink: 0,
    width: 18,
    height: 18,
    borderRadius: "var(--mem-r-5)",
    border: "none",
    background: "none",
    cursor: "pointer",
    color,
  };
}

export default memo(CanvasNode);
