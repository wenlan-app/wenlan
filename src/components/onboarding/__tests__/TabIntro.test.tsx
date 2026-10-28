// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { resetTabIntros, TabIntro as Intro, type IntroTab } from "../TabIntro";

const area = { current: null };
function TabIntro(props: { tab: IntroTab | null; newUser: boolean }) {
  return <Intro {...props} areaRef={area} />;
}

describe("TabIntro", () => {
  beforeEach(() => resetTabIntros());
  afterEach(() => resetTabIntros());

  it("explains a tab the first time a new user opens it, and not after they leave", () => {
    const { rerender } = render(<TabIntro tab="topics" newUser />);
    expect(screen.getByTestId("tab-intro")).toHaveTextContent("Topics: people, places and things your pages mention.");

    rerender(<TabIntro tab={null} newUser />);
    expect(screen.queryByTestId("tab-intro")).toBeNull();
    rerender(<TabIntro tab="topics" newUser />);
    expect(screen.queryByTestId("tab-intro")).toBeNull();

    // Another tab still gets its own line.
    rerender(<TabIntro tab="sources" newUser />);
    expect(screen.getByTestId("tab-intro")).toHaveAttribute("data-tab", "sources");
  });

  it("closes for good with its ×", () => {
    const { unmount } = render(<TabIntro tab="graph" newUser />);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByTestId("tab-intro")).toBeNull();
    unmount();
    render(<TabIntro tab="graph" newUser />);
    expect(screen.queryByTestId("tab-intro")).toBeNull();
  });

  it("says nothing to someone whose library is established", () => {
    render(<TabIntro tab="spaces" newUser={false} />);
    expect(screen.queryByTestId("tab-intro")).toBeNull();
  });

  it("comes back after Settings resets the tips", () => {
    const { unmount } = render(<TabIntro tab="memories" newUser />);
    unmount();
    resetTabIntros();
    render(<TabIntro tab="memories" newUser />);
    expect(screen.getByTestId("tab-intro")).toBeInTheDocument();
  });
});
