// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { CodeBasisRef } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getPageCodeBasisCited: vi.fn(),
  readCodeLines: vi.fn(),
  readGitBlobLines: vi.fn(),
  clipboardWrite: vi.fn(),
}));

import { clipboardWrite, getPageCodeBasisCited, readCodeLines, readGitBlobLines } from "../../../lib/tauri";
import { CodeBasisDialog } from "./CodeBasisDialog";

const BLOB = "0123456789abcdef0123456789abcdef01234567";

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
  vi.mocked(getPageCodeBasisCited).mockReset();
  vi.mocked(readCodeLines).mockReset();
  vi.mocked(readGitBlobLines).mockReset();
  vi.mocked(clipboardWrite).mockReset();
});

describe("CodeBasisDialog", () => {
  it("shows the lines as cited next to the lines now, numbered from the first line", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: BLOB, confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["fn parse() {", "    old();", "}"]);
    vi.mocked(readCodeLines).mockResolvedValue(["fn parse() {", "    new();", "}"]);
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
    expect(getPageCodeBasisCited).toHaveBeenCalledWith("page-1", expect.objectContaining({ line_start: 10, line_end: 12 }));
    expect(readGitBlobLines).toHaveBeenCalledWith("/repo", BLOB, 10, 12);
    expect(readCodeLines).toHaveBeenCalledWith("/repo/src/parser.rs", 10, 62);
    // The rows sit in one sideways scroller, which is the box's only focus target.
    for (const box of [before, after]) {
      expect(box.querySelectorAll("[tabindex]")).toHaveLength(1);
      expect(box.querySelector(".code-basis__scroll > .code-basis__lines")?.children).toHaveLength(3);
    }
  });

  it("tints the lines that exist on one side only and leaves equal lines plain", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: BLOB, confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["fn parse() {", "    old();", "}"]);
    vi.mocked(readCodeLines).mockResolvedValue(["fn parse() {", "    new();", "    more();", "}"]);
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
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: BLOB, confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["a();", "b();"]);
    vi.mocked(readCodeLines).mockResolvedValue(["a();", "b();"]);
    renderDialog();

    await screen.findByTestId("code-basis-after");
    expect(document.querySelectorAll("[data-change]")).toHaveLength(0);
  });

  it("says there is nothing to compare when the lines were never committed", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: null, confirmed_at: 1, state: "pending" });
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
    renderDialog();

    expect(await screen.findByText("The lines as cited were never committed to git, so there is nothing to compare"))
      .toBeInTheDocument();
    expect(await screen.findByTestId("code-basis-after")).toHaveTextContent("new();");
    expect(screen.queryByTestId("code-basis-before")).toBeNull();
    // Nothing is tinted without a cited side, and the message sits above the one column.
    expect(document.querySelectorAll("[data-change]")).toHaveLength(0);
    expect(document.querySelector(".code-basis__columns--single")).not.toBeNull();
    expect(screen.queryByRole("heading", { name: "When cited" })).toBeNull();
    expect(readGitBlobLines).not.toHaveBeenCalled();
  });

  it("says the same when git cannot read the cited version", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: BLOB, confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockRejectedValue("git could not find that version of the file");
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
    renderDialog();

    expect(await screen.findByText(/never committed to git/)).toBeInTheDocument();
    expect(await screen.findByTestId("code-basis-after")).toHaveTextContent("new();");
    expect(screen.queryByTestId("code-basis-before")).toBeNull();
  });

  it("says the file cannot be read now when reading fails, and still shows what was cited", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: BLOB, confirmed_at: 1, state: "unavailable" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["old();"]);
    vi.mocked(readCodeLines).mockRejectedValue("No such file or directory");
    renderDialog({ row: row({ state: "unavailable", reason: "file_missing" }) });

    expect(await screen.findByRole("alert")).toHaveTextContent("The file cannot be read now");
    expect(await screen.findByTestId("code-basis-before")).toHaveTextContent("old();");
    expect(screen.queryByTestId("code-basis-after")).toBeNull();
    // Nothing to confirm for code that cannot be found.
    expect(screen.queryByRole("button", { name: "No impact" })).toBeNull();
  });

  it("confirms, closes, and copies the location for two seconds", async () => {
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: BLOB, confirmed_at: 1, state: "pending" });
    vi.mocked(readGitBlobLines).mockResolvedValue(["old();"]);
    vi.mocked(readCodeLines).mockResolvedValue(["new();"]);
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
    vi.mocked(getPageCodeBasisCited).mockResolvedValue({ cited_blob: null, confirmed_at: 1, state: "pending" });
    vi.mocked(readCodeLines).mockResolvedValue([]);
    const { onClose } = renderDialog();

    fireEvent.keyDown(await screen.findByRole("dialog"), { key: "Escape" });
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
