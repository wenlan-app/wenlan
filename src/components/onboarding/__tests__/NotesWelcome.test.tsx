// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { NotesWelcome } from "../NotesWelcome";
import { exampleWikiActive, setExampleWikiActive } from "../../../lib/exampleWiki/mode";

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

function startButton() {
  return screen.getByTestId("notes-welcome-empty");
}

function sampleButton() {
  return screen.getByTestId("notes-welcome-example");
}

describe("NotesWelcome", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    localStorage.clear();
  });

  it("offers four ways in as rows, and nothing else to fill in", () => {
    render(<NotesWelcome onComplete={vi.fn()} />);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(i18n.t("setup.welcomeTitle"));
    expect(screen.getByText(i18n.t("setup.notesFirst.welcome.lead"))).toBeInTheDocument();
    expect(screen.getAllByRole("button").map((button) => button.dataset.testid)).toEqual([
      "notes-welcome-folder", "notes-welcome-chats", "notes-welcome-empty", "notes-welcome-example",
    ]);
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

});

describe("NotesWelcome ways in", () => {
  beforeEach(() => setExampleWikiActive(false));

  it.each([
    ["folder", "folder"],
    ["chats", "chats"],
    ["empty", "empty"],
  ] as const)("the %s row opens the app and leaves what to do there", async (row, intent) => {
    const { takeStartIntent } = await import("../startIntent");
    const onComplete = vi.fn();
    render(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(screen.getByTestId(`notes-welcome-${row}`));
    await waitFor(() => expect(onComplete).toHaveBeenCalledOnce());
    expect(takeStartIntent()).toBe(intent);
    // Taken once: a later visit to the Wiki opens nothing on its own.
    expect(takeStartIntent()).toBeNull();
  });

  it("the example row opens the app inside the example wiki, with nothing to do there", async () => {
    const { takeStartIntent } = await import("../startIntent");
    const onComplete = vi.fn();
    render(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(screen.getByTestId("notes-welcome-example"));
    await waitFor(() => expect(onComplete).toHaveBeenCalledOnce());
    expect(exampleWikiActive()).toBe(true);
    expect(takeStartIntent()).toBeNull();
  });

  it("leaves the example off when the app could not be opened", async () => {
    const onComplete = vi.fn().mockRejectedValueOnce(new Error("setup failed"));
    render(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(screen.getByTestId("notes-welcome-example"));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(exampleWikiActive()).toBe(false);
  });
});
