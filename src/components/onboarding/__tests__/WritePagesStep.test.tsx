// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import type { Page } from "../../../lib/tauri";
import type { WritePagesState, WritePagesView } from "../../memory/askAi/WritePages";
import { WritePagesStep } from "../WritePagesStep";

function page(id: string): Page {
  return { id, title: `Page ${id}`, creation_kind: "distilled", source_memory_ids: ["m1", "m2", "m3"] } as Page;
}

function state(view: WritePagesView, overrides: Partial<WritePagesState> = {}): WritePagesState {
  return {
    view,
    stepView: view,
    onDemand: true,
    loaded: true,
    run: null,
    tool: { family: "claude-code", name: "Claude Code" },
    toolName: "Claude Code",
    opensTool: true,
    command: "/wenlan steep",
    copied: false,
    waiting: "12 notes and 4 files",
    start: vi.fn(async () => undefined),
    copyCommand: vi.fn(async () => true),
    close: vi.fn(),
    ...overrides,
  };
}

function renderStep(props: Partial<Parameters<typeof WritePagesStep>[0]> & { state: WritePagesState | null }) {
  return render(
    <ul>
      <li data-testid="row">
        <WritePagesStep pages={[]} done={false} {...props} />
      </li>
    </ul>,
  );
}

describe("WritePagesStep", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("names what is waiting and hands it to the tool in one press", () => {
    const ready = state({ kind: "ready", list: { items: [], total: 16 } });
    renderStep({ state: ready });
    expect(screen.getByTestId("row")).toHaveTextContent("New material: 12 notes and 4 files.");
    fireEvent.click(screen.getByRole("button", { name: "Have Claude Code write pages" }));
    expect(ready.start).toHaveBeenCalled();
  });

  it("copies the command for a tool no link can open", () => {
    renderStep({ state: state({ kind: "ready", list: { items: [], total: 16 } }, { opensTool: false }) });
    expect(screen.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
  });

  it("offers the local model when no tool is connected", () => {
    const onUseLocalModel = vi.fn();
    renderStep({ state: state({ kind: "ready", list: { items: [], total: 16 } }, { tool: null }), onUseLocalModel });
    fireEvent.click(screen.getByRole("button", { name: "Use local model" }));
    expect(onUseLocalModel).toHaveBeenCalled();
  });

  it("follows the run in the row and opens a page from it", () => {
    const onSelectPage = vi.fn();
    renderStep({
      state: state({ kind: "working", written: [page("a")], writing: true, stuck: false }),
      done: true,
      onSelectPage,
    });
    expect(screen.getByTestId("row")).toHaveTextContent("Claude Code is writing pages: 1 ready");
    expect(screen.getByTestId("row")).toHaveTextContent("Writing the next page…");
    fireEvent.click(screen.getByRole("button", { name: "Page a" }));
    expect(onSelectPage).toHaveBeenCalledWith("a");
  });

  it("says Wenlan does it in Automatic mode", () => {
    renderStep({ state: state({ kind: "hidden" }, { onDemand: false }) });
    expect(screen.getByTestId("row")).toHaveTextContent(i18n.t("firstUse.checklist.steps.pages.automatic"));
    expect(screen.queryByRole("button")).toBeNull();
  });
});
