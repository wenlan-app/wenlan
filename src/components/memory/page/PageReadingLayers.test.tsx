// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PageReadingLayers, usePageLength, type PageLength } from "./PageReadingLayers";

const PAGE = "The point in one line.\n\n## Why\n\nThe longer reasoning.\n\n## Details for AI\n\nRaw notes for the model.";

function renderLayers(content = PAGE, length: PageLength = "full", onChoose = vi.fn()) {
  render(<PageReadingLayers content={content} length={length} onChoose={onChoose} renderBody={(markdown) => <div data-testid="body">{markdown}</div>} />);
  return onChoose;
}
const bodies = () => screen.getAllByTestId("body").map((node) => node.textContent);

describe("PageReadingLayers", () => {
  beforeEach(() => localStorage.clear());

  it("UX-VIEW-2 shows the first paragraph when short and ends with a way to the full page", () => {
    const onChoose = renderLayers(PAGE, "short");
    expect(bodies()[0]).toBe("The point in one line.");
    fireEvent.click(screen.getByRole("button", { name: "Show the full page" }));
    expect(onChoose).toHaveBeenCalledWith("full");
  });

  it("UX-VIEW-2 puts no length switch above the page body", () => {
    renderLayers();
    expect(bodies()[0]).toContain("The longer reasoning.");
    expect(screen.queryByRole("group", { name: "Length" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Show the full page" })).toBeNull();
  });

  it("UX-VIEW-2 shows the whole page when both lengths are the same", () => {
    renderLayers("One paragraph only.", "short");
    expect(bodies()[0]).toBe("One paragraph only.");
    expect(screen.queryByRole("button", { name: "Show the full page" })).toBeNull();
  });

  it("UX-VIEW-2 remembers the last length for the next page", () => {
    const first = renderHook(() => usePageLength());
    act(() => first.result.current[1]("short"));
    first.unmount();
    expect(renderHook(() => usePageLength()).result.current[0]).toBe("short");
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
