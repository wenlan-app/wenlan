// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { useRef } from "react";
import { PageTour, replayPageTour } from "../PageTour";

const TOUR_KEY = "onboarding:pageTour";

function Page({ unverified = true, chips = true, eligible = true, newUser = true }: {
  unverified?: boolean;
  chips?: boolean;
  eligible?: boolean;
  newUser?: boolean;
}) {
  const root = useRef<HTMLDivElement | null>(null);
  return (
    <div ref={root}>
      <div data-testid="page-document-reading">
        <p>
          A sentence.
          {chips ? <button type="button" className="citation-chip" data-status="verified">1</button> : null}
          {chips && unverified ? <button type="button" className="citation-chip" data-status="unverified">2</button> : null}
        </p>
      </div>
      <PageTour rootRef={root} eligible={eligible} newUser={newUser} pageKey="p1" />
    </div>
  );
}

describe("PageTour", () => {
  beforeEach(() => localStorage.removeItem(TOUR_KEY));
  afterEach(() => localStorage.removeItem(TOUR_KEY));

  it("walks a new user through the three tips, then never again", () => {
    const { unmount } = render(<Page />);
    const tour = screen.getByTestId("page-tour");
    expect(tour).toHaveAttribute("data-tip", "sources");
    expect(tour).toHaveTextContent("1 of 3");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByTestId("page-tour")).toHaveAttribute("data-tip", "unverified");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByTestId("page-tour")).toHaveAttribute("data-tip", "edit");
    expect(screen.getByText(/asks you first/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Got it" }));
    expect(screen.queryByTestId("page-tour")).toBeNull();
    expect(localStorage.getItem(TOUR_KEY)).toBe("seen");
    unmount();
    render(<Page />);
    expect(screen.queryByTestId("page-tour")).toBeNull();
  });

  it("leaves out a tip with nothing to point at", () => {
    render(<Page unverified={false} />);
    expect(screen.getByTestId("page-tour")).toHaveTextContent("1 of 2");
    fireEvent.click(screen.getByRole("button", { name: "Next" }));
    expect(screen.getByTestId("page-tour")).toHaveAttribute("data-tip", "edit");
  });

  it("waits for sources and shows nothing on a page without them", () => {
    render(<Page chips={false} />);
    expect(screen.queryByTestId("page-tour")).toBeNull();
    expect(localStorage.getItem(TOUR_KEY)).toBeNull();
  });

  it("closes on Escape", () => {
    render(<Page />);
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.queryByTestId("page-tour")).toBeNull();
    expect(localStorage.getItem(TOUR_KEY)).toBe("seen");
  });

  it("stays away from someone past getting started and from pages it does not fit", () => {
    const { unmount } = render(<Page newUser={false} />);
    expect(screen.queryByTestId("page-tour")).toBeNull();
    unmount();
    render(<Page eligible={false} />);
    expect(screen.queryByTestId("page-tour")).toBeNull();
  });

  it("comes back once when Settings asks for it", () => {
    localStorage.setItem(TOUR_KEY, "seen");
    render(<Page newUser={false} />);
    expect(screen.queryByTestId("page-tour")).toBeNull();
    act(() => replayPageTour());
    expect(screen.getByTestId("page-tour")).toHaveAttribute("data-tip", "sources");
    fireEvent.click(screen.getByRole("button", { name: "Close tips" }));
    expect(localStorage.getItem(TOUR_KEY)).toBe("seen");
  });
});
