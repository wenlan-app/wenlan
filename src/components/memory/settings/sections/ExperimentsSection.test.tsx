import { act, render, screen, fireEvent } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { setExperiment, useExperiment } from "../../../../lib/experiments";
import { SETTINGS_GROUPS } from "../SettingsSidebar";
import ExperimentsSection from "./ExperimentsSection";

function Probe() {
  const [on] = useExperiment("things");
  return <span data-testid="probe">{on ? "on" : "off"}</span>;
}

describe("Experiments settings", () => {
  afterEach(() => act(() => setExperiment("things", false)));

  it("UX-HOME-1 is its own last Settings tab, says what an experiment is, and turns what you're working on on and off", () => {
    expect(SETTINGS_GROUPS[SETTINGS_GROUPS.length - 1]?.id).toBe("experiments");
    render(<><ExperimentsSection /><Probe /></>);
    expect(screen.getByText(/Turning one off never deletes anything/)).toBeInTheDocument();
    expect(screen.getByTestId("probe")).toHaveTextContent("off");
    fireEvent.click(screen.getByRole("button", { name: "What you're working on" }));
    expect(screen.getByTestId("probe")).toHaveTextContent("on");
  });
});
