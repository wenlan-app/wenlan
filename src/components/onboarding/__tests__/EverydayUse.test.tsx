// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";

vi.mock("../../memory/askAi/askInTool", async (importOriginal) => {
  const actual = await importOriginal<typeof import("../../memory/askAi/askInTool")>();
  return { ...actual, handToTool: vi.fn(async () => "opened") };
});

vi.mock("../../memory/askAi/AskAiDialog", () => ({
  ASK_AI_ICONS: {},
  AskAiDialog: ({ onClose }: { onClose: () => void }) => (
    <div role="dialog" aria-label="What your AI can do">
      <button type="button" onClick={onClose}>Close all commands</button>
    </div>
  ),
}));

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

  it("says these are enough, and opens every command for the curious", async () => {
    // A fresh module: the test before closed the overlay for this session.
    vi.resetModules();
    const { EverydayUse: Fresh } = await import("../EverydayUse");
    render(<Fresh tool={{ family: "claude-code", name: "Claude Code" }} />);
    expect(screen.getByTestId("everyday-use")).toHaveTextContent("These are all you need. Wenlan does the rest.");
    fireEvent.click(screen.getByRole("button", { name: "See all commands" }));
    // The full list takes its place; closing it closes both.
    expect(screen.queryByTestId("everyday-use")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Close all commands" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(localStorage.getItem("onboarding:everydayUse")).toBe("closed");
  });
});
