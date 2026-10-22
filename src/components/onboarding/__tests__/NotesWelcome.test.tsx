// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { NotesWelcome } from "../NotesWelcome";
import { clipboardWrite } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", () => ({ clipboardWrite: vi.fn() }));

function deferred() {
  let resolve!: () => void;
  let reject!: (reason: Error) => void;
  const promise = new Promise<void>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function backButton() {
  return screen.getByRole("button", { name: i18n.t("setup.back") });
}

function startButton() {
  return screen.getByRole("button", { name: i18n.t("setup.notesFirst.start") });
}

function sampleButton() {
  return screen.getByRole("button", { name: i18n.t("firstUse.guide.tryExample") });
}

function enterSampleAi() {
  fireEvent.click(sampleButton());
  fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.sample.skipToResult") }));
  fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.sample.useWithAi") }));
}

describe("NotesWelcome", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it("is a short brand moment: a start button and an example, with no choice to make", () => {
    render(<NotesWelcome onComplete={vi.fn()} />);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(i18n.t("setup.welcomeTitle"));
    expect(screen.getByText(i18n.t("setup.notesFirst.intro"))).toBeInTheDocument();
    expect(screen.getAllByRole("button")).toEqual([startButton(), sampleButton()]);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.queryByRole("radio")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("completes with no destination and stores no start path", async () => {
    const onComplete = vi.fn();
    render(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(startButton());
    await waitFor(() => expect(startButton()).toBeEnabled());
    expect(onComplete).toHaveBeenCalledExactlyOnceWith();
    expect(localStorage.length).toBe(0);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("awaits completion and keeps the action disabled while it is pending", async () => {
    const completion = deferred();
    const onComplete = vi.fn(() => completion.promise);
    render(<NotesWelcome onComplete={onComplete} />);

    fireEvent.click(startButton());
    expect(onComplete).toHaveBeenCalledTimes(1);
    expect(onComplete).toHaveBeenCalledWith();
    expect(startButton()).toBeDisabled();
    expect(sampleButton()).toBeDisabled();
    expect(startButton()).toHaveAttribute("aria-busy", "true");
    expect(screen.getByRole("heading", { level: 1 })).toBeInTheDocument();

    await act(async () => completion.resolve());

    expect(startButton()).toBeEnabled();
    expect(startButton()).toHaveAttribute("aria-busy", "false");
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("guards clicks delivered in the same batch before pending state renders", async () => {
    const completion = deferred();
    const onComplete = vi.fn(() => completion.promise);
    render(<NotesWelcome onComplete={onComplete} />);
    const button = startButton();

    act(() => {
      button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(onComplete).toHaveBeenCalledTimes(1);
    expect(button).toBeDisabled();
    await act(async () => completion.resolve());
  });

  it("shows a failure, then allows a retry and clears the alert while retrying", async () => {
    const first = deferred();
    const retry = deferred();
    const onComplete = vi.fn()
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => retry.promise);
    render(<NotesWelcome onComplete={onComplete} />);

    fireEvent.click(startButton());
    await act(async () => first.reject(new Error("could not save completion")));

    expect(screen.getByRole("alert")).toHaveTextContent(i18n.t("setup.notesFirst.openFailed"));
    expect(startButton()).toBeEnabled();
    expect(screen.getByRole("heading", { level: 1 })).toBeInTheDocument();

    fireEvent.click(startButton());
    expect(onComplete).toHaveBeenCalledTimes(2);
    expect(startButton()).toBeDisabled();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    await act(async () => retry.resolve());
    expect(startButton()).toBeEnabled();
  });

  it("handles synchronous callbacks and synchronous failures", async () => {
    const onComplete = vi.fn()
      .mockImplementationOnce(() => { throw new Error("could not save completion"); })
      .mockImplementationOnce(() => undefined);
    render(<NotesWelcome onComplete={onComplete} />);

    fireEvent.click(startButton());
    expect(screen.getByRole("alert")).toBeInTheDocument();
    expect(startButton()).toBeEnabled();

    fireEvent.click(startButton());
    await waitFor(() => expect(startButton()).toBeEnabled());
    expect(onComplete).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("opens the actual sample directly and returns focus without completing or calling the runtime", () => {
    const onComplete = vi.fn();
    render(<NotesWelcome onComplete={onComplete} />);
    sampleButton().focus();
    fireEvent.click(sampleButton());

    expect(screen.getByTestId("first-use-sample")).toBeInTheDocument();
    expect(screen.getByTestId("sample-phase-0")).toHaveAttribute("data-active", "true");
    expect(screen.queryByRole("heading", { level: 1 })).not.toBeInTheDocument();
    expect(backButton()).toBeEnabled();
    expect(screen.queryByRole("button", { name: i18n.t("setup.notesFirst.start") })).not.toBeInTheDocument();
    fireEvent.click(backButton());

    expect(screen.queryByTestId("first-use-sample")).not.toBeInTheDocument();
    expect(sampleButton()).toHaveFocus();
    expect(onComplete).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
    expect(clipboardWrite).not.toHaveBeenCalled();
  });

  it("has no header action in the sample: the only exits are Back and the final start button", () => {
    render(<NotesWelcome onComplete={vi.fn()} />);
    fireEvent.click(sampleButton());
    expect(screen.queryByRole("button", { name: i18n.t("setup.notesFirst.start") })).not.toBeInTheDocument();
    expect(backButton()).toBeInTheDocument();
  });

  it("keeps the sample behind the citation dialog boundary and restores citation focus", () => {
    const onComplete = vi.fn();
    render(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(sampleButton());
    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.sample.skipToResult") }));
    const citation = screen.getByTestId("sample-citation-sample-cite-1");
    citation.focus();
    fireEvent.click(citation);

    expect(screen.getByTestId("first-use-sample")).toHaveAttribute("inert");
    expect(screen.getByTestId("first-use-sample")).toContainElement(backButton());
    expect(screen.getByRole("dialog")).toHaveFocus();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(citation).toHaveFocus();
    expect(onComplete).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("ends the sample AI panel with one start button that opens notes without importing sample content", async () => {
    const onComplete = vi.fn();
    render(<NotesWelcome onComplete={onComplete} />);
    enterSampleAi();
    expect(screen.queryByRole("button", { name: /Connect my AI|连接我的 AI/ })).not.toBeInTheDocument();
    fireEvent.click(startButton());
    await waitFor(() => expect(startButton()).toBeEnabled());
    expect(onComplete).toHaveBeenCalledExactlyOnceWith();
    expect(invoke).not.toHaveBeenCalled();
    expect(clipboardWrite).not.toHaveBeenCalled();
  });

  it("keeps the selected sample screen after failure and allows a retry", async () => {
    const first = deferred();
    const retry = deferred();
    const onComplete = vi.fn()
      .mockImplementationOnce(() => first.promise)
      .mockImplementationOnce(() => retry.promise);
    render(<NotesWelcome onComplete={onComplete} />);
    enterSampleAi();
    fireEvent.click(screen.getByRole("tab", { name: "Claude" }));
    const start = startButton();
    fireEvent.click(start);
    await act(async () => first.reject(new Error("could not save completion")));

    expect(screen.getByRole("alert")).toHaveTextContent(i18n.t("setup.notesFirst.openFailed"));
    expect(screen.getByRole("tab", { name: "Claude" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("/handoff")).toBeInTheDocument();
    expect(screen.getByTestId("first-use-sample").parentElement).not.toHaveAttribute("inert");
    fireEvent.click(start);
    expect(onComplete).toHaveBeenNthCalledWith(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    await act(async () => retry.resolve());
    expect(startButton()).toBeEnabled();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("guards duplicate and competing sample actions while completion is pending", async () => {
    const completion = deferred();
    const onComplete = vi.fn(() => completion.promise);
    render(<NotesWelcome onComplete={onComplete} />);
    enterSampleAi();
    const start = startButton();
    const back = backButton();
    act(() => {
      start.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      start.dispatchEvent(new MouseEvent("click", { bubbles: true }));
      back.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(onComplete).toHaveBeenCalledExactlyOnceWith();
    expect(screen.getByTestId("first-use-sample").parentElement).toHaveAttribute("inert");
    fireEvent.click(screen.getByRole("tab", { name: "ChatGPT" }));
    expect(screen.getByRole("tab", { name: "Codex" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByTestId("first-use-sample")).toBeInTheDocument();
    await act(async () => completion.resolve());
    expect(screen.getByTestId("first-use-sample").parentElement).not.toHaveAttribute("inert");
    expect(invoke).not.toHaveBeenCalled();
  });
});
