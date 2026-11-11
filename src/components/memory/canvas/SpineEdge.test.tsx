// SPDX-License-Identifier: AGPL-3.0-only
import { render } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

vi.mock("@xyflow/react", () => ({
  BaseEdge: ({ path }: { path: string }) => <path data-testid="edge" d={path} />,
}));

import SpineEdge from "./SpineEdge";

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
