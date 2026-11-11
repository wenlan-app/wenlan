// SPDX-License-Identifier: AGPL-3.0-only
import { BaseEdge, type Edge, type EdgeProps } from "@xyflow/react";

/**
 * The stroke between a box and its parent, drawn as a mind map draws it.
 *
 * React Flow's own bezier is wrong here, and not by a little: it derives its
 * control points from the handle's `Position` enum, so with our handles pinned
 * at the box center as Top and Bottom every curve leaves the parent going down
 * and enters the child going down. On a radial map, where a child can sit at
 * any angle, that reads as an S laid over the spoke. `straight` avoided the S
 * but drew the map as a wire diagram.
 *
 * A quadratic with its control point pushed off the chord's own midpoint has no
 * opinion about which way is down: the arc follows wherever the child actually
 * is, which is the only direction that means anything on a ring.
 */

/**
 * How far the arc bows, as a fraction of the distance between the two boxes.
 *
 * Proportional rather than fixed so a long spoke to the outer ring and a short
 * one between siblings bow by the same visual amount. Kept small on purpose:
 * boxes on one ring are separated by their own width, not by room for a curve,
 * so a deep bow on a crowded ring will lean a stroke into the neighbouring box.
 */
const BOW = 0.11;

/**
 * The most the drawn arc may sag away from the straight line, px.
 *
 * A proportional bow is right for the spans the layout produces — a ring is one
 * box-depth out from the last, so spokes run 150-250px and sag 8-14px. It stops
 * being right once a box has been dragged: pull a child 600px out and a purely
 * proportional bow sags 33px, which stops reading as a curve and starts reading
 * as a detour around something.
 */
const MAX_SAG = 24;

function SpineEdge({
  sourceX,
  sourceY,
  targetX,
  targetY,
  style,
  markerEnd,
  data,
}: EdgeProps) {
  // On the left-to-right tree the line leaves its parent sideways and arrives
  // at its child sideways, so siblings fan out like an outline's indents.
  const shape = data as
    | {
        tree?: boolean;
        indent?: { fromX: number; fromY: number; toX: number };
        /** Where the tree line meets the boxes, when not at their centers. */
        ends?: { fromX: number; fromY: number; toX: number; toY: number };
      }
    | undefined;
  // On the indented outline a line drops from under the parent's first letter
  // and turns right into the child, as an outline's guide lines do.
  if (shape?.indent) {
    const { fromX, fromY, toX } = shape.indent;
    return (
      <BaseEdge
        path={`M${fromX},${fromY} V${targetY - 6} Q${fromX},${targetY} ${fromX + 6},${targetY} H${toX}`}
        style={style}
        markerEnd={markerEnd}
      />
    );
  }
  if (shape?.tree) {
    // A line drawn as text on a stroke is met at the start of that stroke, so
    // the branch runs on into it instead of striking through the words.
    const sx = shape.ends?.fromX ?? sourceX;
    const sy = shape.ends?.fromY ?? sourceY;
    const tx = shape.ends?.toX ?? targetX;
    const ty = shape.ends?.toY ?? targetY;
    const mx = (sx + tx) / 2;
    return (
      <BaseEdge
        path={`M${sx},${sy} C${mx},${sy} ${mx},${ty} ${tx},${ty}`}
        style={style}
        markerEnd={markerEnd}
      />
    );
  }
  const dx = targetX - sourceX;
  const dy = targetY - sourceY;
  const span = Math.hypot(dx, dy);
  // Perpendicular to the chord, one consistent rotational sense for every edge
  // on the map. Mirroring the sign so a fan bows symmetrically is what a static
  // bilateral mind map does, and it is wrong for a canvas you can edit: the sign
  // flips the moment a child is dragged across its fan's bisector, so the arc
  // visibly snaps to the other side mid-drag. One sense is stable under every
  // edit, and the slight pinwheel reads as deliberate.
  //
  // The drawn arc sags half this offset — the midpoint of a quadratic sits
  // halfway to its control point — so the cap is doubled here.
  const offset = Math.min(span * BOW, MAX_SAG * 2);
  const cx = (sourceX + targetX) / 2 + (-dy / (span || 1)) * offset;
  const cy = (sourceY + targetY) / 2 + (dx / (span || 1)) * offset;

  return (
    <BaseEdge
      path={`M${sourceX},${sourceY} Q${cx},${cy} ${targetX},${targetY}`}
      style={style}
      markerEnd={markerEnd}
    />
  );
}

export default SpineEdge;
export type SpineEdgeType = Edge<Record<string, unknown>, "spine">;
