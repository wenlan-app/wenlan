// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import { NotesWelcome } from "../NotesWelcome";
import { exampleWikiActive, setExampleWikiActive } from "../../../lib/exampleWiki/mode";

const tauri = vi.hoisted(() => ({
  detectMcpClients: vi.fn(),
  writeMcpConfig: vi.fn(),
  installClientPlugin: vi.fn(),
  installClientSkills: vi.fn(),
}));
vi.mock("../../../lib/tauri", () => ({ clipboardWrite: vi.fn(), ...tauri }));

function renderQ(ui: React.ReactElement) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const result = render(<QueryClientProvider client={client}>{ui}</QueryClientProvider>);
  return { ...result, rerender: (next: React.ReactElement) => result.rerender(<QueryClientProvider client={client}>{next}</QueryClientProvider>) };
}

const yes = { kind: "yes" } as const;
const no = { kind: "no" } as const;
function client(type: string, configured = false) {
  return {
    name: type, client_type: type, config_path: null, detected: yes,
    already_configured: configured ? yes : no, has_raw_entry: no, has_raw_duplicate: no, has_plugin: configured ? yes : no,
  };
}

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
    sessionStorage.clear();
    tauri.detectMcpClients.mockResolvedValue([]);
  });

  it("offers three ways in and the example as a link, then says in one quiet line who writes the pages, and nothing else to fill in", () => {
    renderQ(<NotesWelcome onComplete={vi.fn()} />);

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent(i18n.t("setup.welcomeTitle"));
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.getByTestId("notes-welcome-writer-line")).toBeInTheDocument();
    expect(screen.getByText(i18n.t("setup.notesFirst.welcome.lead"))).toBeInTheDocument();
    expect(screen.getAllByRole("button").map((button) => button.dataset.testid)).toEqual([
      "notes-welcome-folder", "notes-welcome-chats", "notes-welcome-empty", "notes-welcome-example", "notes-welcome-writer-change",
    ]);
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("combobox")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(invoke).not.toHaveBeenCalled();
  });

  it("with no AI tool found, defaults to keeping material without downloading anything", async () => {
    renderQ(<NotesWelcome onComplete={vi.fn()} />);
    await waitFor(() => expect(tauri.detectMcpClients).toHaveBeenCalled());
    expect(screen.getByTestId("notes-welcome-writer-line")).toHaveTextContent("What you add is kept and gets organized once you connect an AI tool.");
    fireEvent.click(screen.getByTestId("notes-welcome-writer-change"));
    expect(screen.getByRole("radiogroup", { name: i18n.t("setup.notesFirst.writer.question") })).toBeInTheDocument();
    expect(screen.queryByTestId("notes-welcome-writer-tool")).toBeNull();
    expect(screen.getByTestId("notes-welcome-writer-none")).toHaveAttribute("aria-checked", "true");
    expect(screen.getByTestId("notes-welcome-writer-local")).toHaveAttribute("aria-checked", "false");
  });

  it("defaults to Claude Code when it is on this computer, says it writes pages next time, and connects it on start", async () => {
    tauri.detectMcpClients.mockResolvedValue([client("codex_cli"), client("claude_code")]);
    const onComplete = vi.fn();
    renderQ(<NotesWelcome onComplete={onComplete} />);
    expect(await screen.findByText("Claude Code (found on this computer) turns what you add into pages.")).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("notes-welcome-writer-change"));
    const tool = screen.getByTestId("notes-welcome-writer-tool");
    expect(tool).toHaveAttribute("aria-checked", "true");
    expect(tool).toHaveTextContent("Claude Code (found on this computer)");
    expect(tool).toHaveTextContent("Next time you use Claude Code, it turns what you add into pages.");
    fireEvent.click(startButton());
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(tauri.installClientPlugin).toHaveBeenCalledWith("claude_code");
    expect(JSON.parse(localStorage.getItem("askAi:writePages:tool") ?? "null")).toBe("claude-code");
  });

  it("does not set up a tool that is already connected", async () => {
    tauri.detectMcpClients.mockResolvedValue([client("claude_code", true)]);
    const onComplete = vi.fn();
    renderQ(<NotesWelcome onComplete={onComplete} />);
    await screen.findByText("Claude Code (found on this computer) turns what you add into pages.");
    fireEvent.click(startButton());
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(tauri.installClientPlugin).not.toHaveBeenCalled();
  });

  it("a tool that fails to connect still opens the app", async () => {
    tauri.detectMcpClients.mockResolvedValue([client("claude_code")]);
    tauri.installClientPlugin.mockRejectedValue(new Error("read-only"));
    const onComplete = vi.fn();
    renderQ(<NotesWelcome onComplete={onComplete} />);
    await screen.findByText("Claude Code (found on this computer) turns what you add into pages.");
    fireEvent.click(startButton());
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("the local model downloads nothing here: it only asks home to open the model choice once", async () => {
    tauri.detectMcpClients.mockResolvedValue([client("claude_code")]);
    const onComplete = vi.fn();
    renderQ(<NotesWelcome onComplete={onComplete} />);
    await screen.findByText("Claude Code (found on this computer) turns what you add into pages.");
    fireEvent.click(screen.getByTestId("notes-welcome-writer-change"));
    fireEvent.click(screen.getByTestId("notes-welcome-writer-local"));
    expect(screen.getByTestId("notes-welcome-writer-local")).toHaveAttribute("aria-checked", "true");
    fireEvent.click(startButton());
    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(tauri.installClientPlugin).not.toHaveBeenCalled();
    expect(invoke).not.toHaveBeenCalled();
    expect(sessionStorage.getItem("onboarding:writerLocal")).toBe("1");
  });

  it("completes with no destination and stores no start path", async () => {
    const onComplete = vi.fn();
    renderQ(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(startButton());
    await waitFor(() => expect(startButton()).toBeEnabled());
    expect(onComplete).toHaveBeenCalledExactlyOnceWith();
    expect(localStorage.length).toBe(0);
    expect(invoke).not.toHaveBeenCalled();
  });

  it("awaits completion and keeps the action disabled while it is pending", async () => {
    const completion = deferred();
    const onComplete = vi.fn(() => completion.promise);
    renderQ(<NotesWelcome onComplete={onComplete} />);

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
    renderQ(<NotesWelcome onComplete={onComplete} />);
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
    renderQ(<NotesWelcome onComplete={onComplete} />);

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
    renderQ(<NotesWelcome onComplete={onComplete} />);

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
    renderQ(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(screen.getByTestId(`notes-welcome-${row}`));
    await waitFor(() => expect(onComplete).toHaveBeenCalledOnce());
    expect(takeStartIntent()).toBe(intent);
    // Taken once: a later visit to the Wiki opens nothing on its own.
    expect(takeStartIntent()).toBeNull();
  });

  it("the example row opens the app inside the example wiki, with nothing to do there", async () => {
    const { takeStartIntent } = await import("../startIntent");
    const onComplete = vi.fn();
    renderQ(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(screen.getByTestId("notes-welcome-example"));
    await waitFor(() => expect(onComplete).toHaveBeenCalledOnce());
    expect(exampleWikiActive()).toBe(true);
    expect(takeStartIntent()).toBeNull();
  });

  it("leaves the example off when the app could not be opened", async () => {
    const onComplete = vi.fn().mockRejectedValueOnce(new Error("setup failed"));
    renderQ(<NotesWelcome onComplete={onComplete} />);
    fireEvent.click(screen.getByTestId("notes-welcome-example"));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(exampleWikiActive()).toBe(false);
  });
});
