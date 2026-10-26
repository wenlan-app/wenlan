// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import type { Page } from "../../../lib/tauri";
import type { WritePagesState, WritePagesView } from "../../memory/askAi/WritePages";
import { WritePagesStep } from "../WritePagesStep";

function page(id: string, createdAt = "2026-10-22T10:01:00Z", kind = "distilled"): Page {
  return { id, title: `Page ${id}`, creation_kind: kind, created_at: createdAt, source_memory_ids: ["m1", "m2", "m3"] } as Page;
}

function state(view: WritePagesView, overrides: Partial<WritePagesState> = {}): WritePagesState {
  return {
    view,
    stepView: view,
    cardView: view,
    waitingTotal: 16,
    onDemand: true,
    loaded: true,
    run: null,
    tool: { family: "claude-code", name: "Claude Code" },
    tools: [{ family: "claude-code", name: "Claude Code" }],
    chooseTool: vi.fn(),
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

  it("copies the command for a tool no link can open, and says where it goes", () => {
    renderStep({ state: state({ kind: "ready", list: { items: [], total: 16 } }, { opensTool: false }) });
    expect(screen.getByRole("button", { name: "Copy command" })).toBeInTheDocument();
    expect(screen.getByTestId("row")).toHaveTextContent("Then paste it into Claude Code and press Enter.");
  });

  it("offers the local model when no tool is connected", () => {
    const onUseLocalModel = vi.fn();
    renderStep({ state: state({ kind: "ready", list: { items: [], total: 16 } }, { tool: null }), onUseLocalModel });
    fireEvent.click(screen.getByRole("button", { name: "Use local model" }));
    expect(onUseLocalModel).toHaveBeenCalled();
  });

  it("follows the run in the row and opens a page from it", () => {
    const onSelectPage = vi.fn();
    const run = { startedAt: Date.parse("2026-10-22T10:00:00Z"), tool: "Claude Code", handedOver: "opened" as const };
    renderStep({
      state: state({ kind: "working", written: [page("a")], writing: true, stuck: false }, { run }),
      pages: [page("old", "2026-10-20T10:00:00Z"), page("a")],
      done: true,
      onSelectPage,
    });
    expect(screen.getByTestId("row")).toHaveTextContent("Claude Code is writing pages: 1 ready");
    expect(screen.getByTestId("row")).toHaveTextContent("Writing the next page…");
    // Earlier pages stay listed under the new one.
    expect(screen.getByRole("button", { name: "Page old" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Page a" }));
    expect(onSelectPage).toHaveBeenCalledWith("a");
  });

  it("offers the copy fallback only while nothing has landed", () => {
    const run = { startedAt: Date.parse("2026-10-22T10:00:00Z"), tool: "Claude Code", handedOver: "opened" as const };
    const working = state({ kind: "working", written: [], writing: false, stuck: false }, { run });
    renderStep({ state: working, onSelectPage: vi.fn() });
    fireEvent.click(screen.getByRole("button", { name: "Didn't open? Copy the command" }));
    expect(working.copyCommand).toHaveBeenCalled();
  });

  it("keeps the newest pages in the finished step, with what came in since", () => {
    const ready = state({ kind: "ready", list: { items: [], total: 4 } });
    renderStep({ state: ready, pages: [page("a"), page("mine", "2026-10-22T11:00:00Z", "imported")], done: true, onSelectPage: vi.fn() });
    expect(screen.getByRole("button", { name: "Page a" })).toBeInTheDocument();
    // Copies of brought-in files are not pages to open from here.
    expect(screen.queryByRole("button", { name: "Page mine" })).toBeNull();
    expect(screen.getByTestId("row")).toHaveTextContent("12 notes and 4 files aren't in any page yet.");
    fireEvent.click(screen.getByRole("button", { name: "Have Claude Code write pages" }));
    expect(ready.start).toHaveBeenCalled();
  });

  it("after a run, offers the next batch while material still waits", () => {
    const written = Array.from({ length: 8 }, (_, i) => page(`w${i}`));
    const done = state(
      { kind: "done", written, updated: [], minutes: 4, left: 9943 },
      { waitingTotal: 9943, waiting: "9,943 notes", run: { startedAt: Date.parse("2026-10-22T10:00:00Z"), tool: "Claude Code" } },
    );
    renderStep({ state: done, pages: written, done: true, onSelectPage: vi.fn() });
    expect(screen.getByTestId("getting-started-pages-more")).toHaveTextContent("9,943 notes aren't in any page yet.");
    fireEvent.click(screen.getByRole("button", { name: "Have Claude Code write pages" }));
    expect(done.start).toHaveBeenCalled();
  });

  it("after a run, still offers the next batch with no tool known, by copying the command", () => {
    const written = [page("a")];
    const done = state({ kind: "done", written, updated: [], minutes: 2, left: 40 }, { tool: null, waiting: "40 notes" });
    renderStep({ state: done, pages: written, done: true });
    fireEvent.click(screen.getByRole("button", { name: "Copy command" }));
    expect(done.start).toHaveBeenCalled();
  });

  it("after a run with nothing left, offers nothing more", () => {
    const written = [page("a")];
    renderStep({ state: state({ kind: "done", written, updated: [], minutes: 2, left: 0 }, { waitingTotal: 0 }), pages: written, done: true });
    expect(screen.queryByTestId("getting-started-pages-more")).toBeNull();
  });

  it("once done, offers even a little material still waiting", () => {
    const ready = state({ kind: "hidden" }, { waitingTotal: 2, waiting: "2 notes" });
    renderStep({ state: ready, pages: [page("a")], done: true });
    expect(screen.getByTestId("getting-started-pages-more")).toHaveTextContent("2 notes aren't in any page yet.");
  });

  it("says the last run stopped before writing pages, over the same button", () => {
    const ready = state({ kind: "ready", list: { items: [], total: 16 } }, { stopped: true });
    renderStep({ state: ready });
    expect(screen.getByTestId("getting-started-pages-stopped")).toHaveTextContent("The last run stopped before writing pages.");
    fireEvent.click(screen.getByRole("button", { name: "Have Claude Code write pages" }));
    expect(ready.start).toHaveBeenCalled();
  });

  it("says the tool may be waiting while it holds material and nothing lands", () => {
    renderStep({ state: state({ kind: "working", written: [], writing: true, stuck: true }, { run: { startedAt: Date.now(), tool: "Claude Code", handedOver: "opened" } }) });
    expect(screen.getByTestId("row")).toHaveTextContent("Claude Code is writing pages");
    expect(screen.getByTestId("row")).toHaveTextContent("Nothing happening? Check whether Claude Code is waiting for you.");
  });

  it("says Wenlan does it in Automatic mode", () => {
    renderStep({ state: state({ kind: "hidden" }, { onDemand: false }) });
    expect(screen.getByTestId("row")).toHaveTextContent(i18n.t("firstUse.checklist.steps.pages.automatic"));
    expect(screen.queryByRole("button")).toBeNull();
  });
});
