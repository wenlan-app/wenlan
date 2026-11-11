// SPDX-License-Identifier: AGPL-3.0-only
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@xyflow/react", () => ({
  BaseEdge: ({ path }: { path: string }) => <path data-testid="edge" d={path} />,
}));

import SpineEdge, { outerArc } from "./SpineEdge";

function pathOf(data: unknown) {
  const props = { sourceX: 0, sourceY: 0, targetX: 100, targetY: 50, data } as unknown as Parameters<typeof SpineEdge>[0];
  const { getByTestId } = render(<svg><SpineEdge {...props} /></svg>);
  return getByTestId("edge").getAttribute("d");
}

describe("SpineEdge indent", () => {
  it("turns into the child at its middle by default", () => {
    expect(pathOf({ indent: { fromX: 10, fromY: 20, toX: 40 } })).toBe("M10,20 V44 Q10,50 16,50 H40");
  });

  it("turns into a line drawn on a stroke at the stroke, so the two meet", () => {
    expect(pathOf({ indent: { fromX: 10, fromY: 20, toX: 40, toY: 62 } })).toBe("M10,20 V56 Q10,62 16,62 H40");
  });
});

describe("SpineEdge same-page arc", () => {
  it("UX-VIEW-9 bows out to the given x and comes back, with its apex exactly there", () => {
    const a = { x: 100, y: 20 };
    const b = { x: 140, y: 220 };
    const arc = outerArc(a, b, 400);
    // The control x is solved so the curve's x at t = 1/2 is the outer x.
    const c = Number(/C([\d.-]+),/.exec(arc.path)![1]);
    expect((a.x + b.x) / 8 + (3 * c) / 4).toBeCloseTo(400);
    expect(arc.apex).toEqual({ x: 400, y: 120 });
    expect(arc.path.startsWith("M100,20 ")).toBe(true);
    expect(arc.path.endsWith(" 140,220")).toBe(true);
  });

  it("UX-VIEW-9 draws that arc for an edge that carries an outer route", () => {
    const d = pathOf({ outer: { from: { x: 10, y: 0 }, to: { x: 10, y: 60 }, x: 200 } });
    expect(d).toBe(outerArc({ x: 10, y: 0 }, { x: 10, y: 60 }, 200).path);
  });
});
