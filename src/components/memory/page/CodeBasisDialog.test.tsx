// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { CodeBasisRef } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getPageCodeBasisLines: vi.fn(),
  clipboardWrite: vi.fn(),
}));

import { clipboardWrite, getPageCodeBasisLines } from "../../../lib/tauri";
import { CodeBasisDialog } from "./CodeBasisDialog";

/** What the daemon answers: the lines as cited (null: not in git) and now (null: unreadable). */
function lines(cited: string[] | null, now: string[] | null, citedStart?: number) {
  vi.mocked(getPageCodeBasisLines).mockResolvedValue({ cited, now, cited_start: citedStart });
}

const row = (over: Partial<CodeBasisRef> = {}): CodeBasisRef => ({
  repo_root: "/repo", path: "src/parser.rs", line_start: 10, line_end: 12, state: "pending", ...over,
});

function renderDialog(props: Partial<React.ComponentProps<typeof CodeBasisDialog>> = {}) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onConfirm = vi.fn();
  const onClose = vi.fn();
  render(<QueryClientProvider client={client}>
    <CodeBasisDialog pageId="page-1" row={row()} onConfirm={onConfirm} onClose={onClose} {...props} />
  </QueryClientProvider>);
  return { onConfirm, onClose };
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.mocked(getPageCodeBasisLines).mockReset();
  vi.mocked(clipboardWrite).mockReset();
});

describe("CodeBasisDialog", () => {
  it("shows the lines as cited next to the lines now, numbered from the first line", async () => {
    lines(["fn parse() {", "    old();", "}"], ["fn parse() {", "    new();", "}"], 10);
    renderDialog();

    const dialog = await screen.findByRole("dialog", { name: "src/parser.rs lines 10–12" });
    const before = await within(dialog).findByTestId("code-basis-before");
    const after = await within(dialog).findByTestId("code-basis-after");
    expect(within(dialog).getByRole("heading", { name: "When cited" })).toBeInTheDocument();
    expect(within(dialog).getByRole("heading", { name: "Now" })).toBeInTheDocument();
    expect(before).toHaveTextContent("old();");
    expect(before).not.toHaveTextContent("new();");
    expect(after).toHaveTextContent("new();");
    expect(before.querySelectorAll(".code-basis__line")).toHaveLength(3);
    expect(Array.from(after.querySelectorAll(".code-basis__number")).map((node) => node.textContent))
      .toEqual(["10", "11", "12"]);
    expect(getPageCodeBasisLines).toHaveBeenCalledWith("page-1", expect.objectContaining({ line_start: 10, line_end: 12 }), 50);
    // The rows sit in one sideways scroller, which is the box's only focus target.
    for (const box of [before, after]) {
      expect(box.querySelectorAll("[tabindex]")).toHaveLength(1);
      expect(box.querySelector(".code-basis__scroll > .code-basis__lines")?.children).toHaveLength(3);
    }
  });

  it("tints the lines that exist on one side only and leaves equal lines plain", async () => {
    lines(["fn parse() {", "    old();", "}"], ["fn parse() {", "    new();", "    more();", "}"]);
    renderDialog();

    const before = await screen.findByTestId("code-basis-before");
    const after = await screen.findByTestId("code-basis-after");
    const changes = (el: HTMLElement) =>
      Array.from(el.querySelectorAll(".code-basis__line")).map((line) => line.getAttribute("data-change"));
    expect(changes(before)).toEqual([null, "removed", null]);
    expect(changes(after)).toEqual([null, "added", "added", null]);
    expect(before.querySelector(".code-basis__line--removed")).toHaveTextContent("old();");
    expect(after.querySelector(".code-basis__line--added")).toHaveTextContent("new();");
  });

  it("marks nothing when the lines read the same", async () => {
    lines(["a();", "b();"], ["a();", "b();"]);
    renderDialog();

    await screen.findByTestId("code-basis-after");
    expect(document.querySelectorAll("[data-change]")).toHaveLength(0);
  });

  it("says there is nothing to compare when the lines were never committed", async () => {
    lines(null, ["new();"]);
    renderDialog();

    expect(await screen.findByText("The lines as cited were never committed to git, so there is nothing to compare"))
      .toBeInTheDocument();
    expect(await screen.findByTestId("code-basis-after")).toHaveTextContent("new();");
    expect(screen.queryByTestId("code-basis-before")).toBeNull();
    // Nothing is tinted without a cited side, and the message sits above the one column.
    expect(document.querySelectorAll("[data-change]")).toHaveLength(0);
    expect(document.querySelector(".code-basis__columns--single")).not.toBeNull();
    expect(screen.queryByRole("heading", { name: "When cited" })).toBeNull();
  });

  it("numbers the cited side from where the lines sat in the cited version", async () => {
    // A step was added above: the row now spans the new line and its own.
    lines(["3. Run tests"], ["3. Check Windows", "3. Run tests"], 11);
    renderDialog({ row: row({ line_start: 11, line_end: 12, reason: "lines_added" }) });

    const before = await screen.findByTestId("code-basis-before");
    const after = await screen.findByTestId("code-basis-after");
    const numbers = (el: HTMLElement) =>
      Array.from(el.querySelectorAll(".code-basis__number")).map((node) => node.textContent);
    expect(numbers(before)).toEqual(["11"]);
    expect(numbers(after)).toEqual(["11", "12"]);
    expect(after.querySelector(".code-basis__line--added")).toHaveTextContent("3. Check Windows");
  });

  it("says the file cannot be read now only when the daemon could not read it", async () => {
    lines(["old();"], null);
    renderDialog();

    expect(await screen.findByRole("alert")).toHaveTextContent("The file cannot be read now");
    expect(await screen.findByTestId("code-basis-before")).toHaveTextContent("old();");
  });

  it("says the file cannot be read now when reading fails, and still shows what was cited", async () => {
    lines(["old();"], null);
    renderDialog({ row: row({ state: "unavailable", reason: "file_missing" }) });

    expect(await screen.findByRole("alert")).toHaveTextContent("The file cannot be read now");
    expect(await screen.findByTestId("code-basis-before")).toHaveTextContent("old();");
    expect(screen.queryByTestId("code-basis-after")).toBeNull();
    // Nothing to confirm for code that cannot be found.
    expect(screen.queryByRole("button", { name: "No impact" })).toBeNull();
  });

  it("confirms, closes, and copies the location for two seconds", async () => {
    lines(["old();"], ["new();"]);
    vi.mocked(clipboardWrite).mockResolvedValue(undefined);
    const { onConfirm, onClose } = renderDialog();
    await screen.findByTestId("code-basis-after");

    vi.useFakeTimers({ shouldAdvanceTime: true });
    fireEvent.click(screen.getByRole("button", { name: "Copy location" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    expect(clipboardWrite).toHaveBeenCalledWith("src/parser.rs:10");
    act(() => { vi.advanceTimersByTime(2_100); });
    await waitFor(() => expect(screen.getByRole("button", { name: "Copy location" })).toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "No impact" }));
    expect(onConfirm).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it("closes on Escape", async () => {
    lines(null, []);
    const { onClose } = renderDialog();

    fireEvent.keyDown(await screen.findByRole("dialog"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
