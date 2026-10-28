// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../memory/askAi/askInTool", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../memory/askAi/askInTool")>();
  return { ...actual, handToTool: vi.fn(async () => "opened") };
});

import { EverydayUse } from "../EverydayUse";

describe("EverydayUse", () => {
  afterEach(() => localStorage.removeItem("onboarding:everydayUse"));

  it("gives the three everyday sentences through the front door of a plugin tool", () => {
    render(<EverydayUse tool={{ family: "codex", name: "Codex" }} />);
    expect(screen.getByTestId("everyday-use-start")).toHaveTextContent("$wenlan where did I leave off?");
    expect(screen.getByTestId("everyday-use-remember")).toHaveTextContent("$wenlan remember we switched to SQLite");
    expect(screen.getByTestId("everyday-use-wrapUp")).toHaveTextContent("$wenlan wrap up");
    // The skill that does the same, for people who like the short form.
    expect(screen.getByTestId("everyday-use-start")).toHaveTextContent("or $brief");
    expect(screen.getByTestId("everyday-use-wrapUp")).toHaveTextContent("or $handoff");
  });

  it("asks other tools in plain sentences", () => {
    render(<EverydayUse tool={{ family: "cursor", name: "Cursor" }} />);
    expect(screen.getByTestId("everyday-use-wrapUp")).toHaveTextContent("Ask Wenlan to save where I stopped");
    expect(screen.queryByText(/wenlan wrap up/)).toBeNull();
    expect(screen.queryByText(/handoff/)).toBeNull();
  });

  it("stays closed once closed", () => {
    const { unmount } = render(<EverydayUse tool={{ family: "claude-code", name: "Claude Code" }} />);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByTestId("everyday-use")).toBeNull();
    unmount();
    render(<EverydayUse tool={{ family: "claude-code", name: "Claude Code" }} />);
    expect(screen.queryByTestId("everyday-use")).toBeNull();
  });
});
