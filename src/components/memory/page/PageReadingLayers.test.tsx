// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import { PageReadingLayers } from "./PageReadingLayers";

const PAGE = "The point in one line.\n\n## Why\n\nThe longer reasoning.\n\n## Details for AI\n\nRaw notes for the model.";

function renderLayers(content = PAGE) {
  return render(<PageReadingLayers content={content} renderBody={(markdown) => <div data-testid="body">{markdown}</div>} />);
}

describe("PageReadingLayers", () => {
  beforeEach(() => localStorage.clear());

  it("UX-VIEW-2 switches between short and full on the same page and remembers the choice", () => {
    const { unmount } = renderLayers();
    const bodies = () => screen.getAllByTestId("body").map((node) => node.textContent);
    expect(bodies()[0]).toContain("The longer reasoning.");

    fireEvent.click(screen.getByRole("button", { name: "Short" }));
    expect(screen.getByRole("button", { name: "Short" })).toHaveAttribute("aria-pressed", "true");
    expect(bodies()[0]).toBe("The point in one line.");
    unmount();

    renderLayers();
    expect(screen.getByRole("button", { name: "Short" })).toHaveAttribute("aria-pressed", "true");
  });

  it("UX-VIEW-2 marks the length switch as Beta", () => {
    renderLayers();
    expect(screen.getByRole("group", { name: "Length" }).parentElement).toHaveTextContent("Beta");
  });

  it("UX-VIEW-2 offers no switch when both lengths are the same", () => {
    renderLayers("One paragraph only.");
    expect(screen.queryByRole("group", { name: "Length" })).toBeNull();
  });

  it("UX-VIEW-3 folds the part for AI below the human part, closed", () => {
    renderLayers();
    const fold = screen.getByTestId("page-ai-details");
    expect(fold).not.toHaveAttribute("open");
    expect(fold).toHaveTextContent("Details for AI");
    expect(fold).toHaveTextContent("Raw notes for the model.");
    expect(screen.getAllByTestId("body")[0]).not.toHaveTextContent("Raw notes for the model.");
  });
});
