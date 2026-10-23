// SPDX-License-Identifier: AGPL-3.0-only
import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, fireEvent, act, within } from "@testing-library/react";
import { FirstUseSample } from "../FirstUseSample";
import { i18n } from "../../../i18n";

vi.mock("../../../lib/tauri", () => ({
  clipboardWrite: vi.fn(),
}));

import { invoke } from "@tauri-apps/api/core";
import { clipboardWrite } from "../../../lib/tauri";

const mockedInvoke = vi.mocked(invoke);
const mockedClipboardWrite = vi.mocked(clipboardWrite);

function renderSample(props = {}) {
  return render(
    <FirstUseSample
      onBack={vi.fn()}
      onFinish={vi.fn()}
      {...props}
    />,
  );
}

function skipToResult() {
  fireEvent.click(screen.getByRole("button", { name: "Skip to result" }));
}

function openAiPanel() {
  skipToResult();
  fireEvent.click(screen.getByRole("button", { name: "Commands for later" }));
}

function stubMatchMedia(initial: boolean) {
  const original = window.matchMedia;
  let matches = initial;
  const listeners = new Set<(event: { matches: boolean }) => void>();
  const mq = {
    get matches() {
      return matches;
    },
    media: "(prefers-reduced-motion: reduce)",
    addEventListener: vi.fn((_type: string, cb: (event: { matches: boolean }) => void) => {
      listeners.add(cb);
    }),
    removeEventListener: vi.fn((_type: string, cb: (event: { matches: boolean }) => void) => {
      listeners.delete(cb);
    }),
  };
  window.matchMedia = vi.fn().mockReturnValue(mq);
  return {
    mq,
    setMatches(next: boolean) {
      matches = next;
      listeners.forEach((cb) => cb({ matches: next }));
    },
    restore() {
      window.matchMedia = original;
    },
  };
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  vi.clearAllMocks();
  mockedClipboardWrite.mockResolvedValue(undefined);
});

afterEach(async () => {
  vi.useRealTimers();
  await i18n.changeLanguage("en");
});

describe("FirstUseSample demo isolation", () => {
  it("issues no daemon calls while walking the whole sample", async () => {
    renderSample();
    skipToResult();
    expect(
      screen.getByText("v2 launch plan"),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByTestId("sample-citation-sample-cite-2"));
    expect(screen.getByTestId("sample-citation-dialog")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(screen.getByRole("button", { name: "Commands for later" }));
    expect(screen.getByRole("heading", { name: "Commands for later" })).toBeInTheDocument();
    // The only daemon-adjacent call allowed on the sample path is clipboard.
    expect(mockedInvoke).not.toHaveBeenCalled();
  });

  it("autoplays through phases and cleans its timer up on unmount", async () => {
    vi.useFakeTimers();
    const clearSpy = vi.spyOn(window, "clearTimeout");
    const { unmount } = renderSample();
    expect(screen.getByTestId("sample-phase-0")).toHaveAttribute(
      "data-active",
      "true",
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(screen.getByTestId("sample-phase-1")).toHaveAttribute(
      "data-active",
      "true",
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    await act(async () => {
      await vi.advanceTimersByTimeAsync(3000);
    });
    expect(screen.getByTestId("sample-phase-3")).toHaveAttribute(
      "data-active",
      "true",
    );
    unmount();
    expect(clearSpy).toHaveBeenCalled();
    clearSpy.mockRestore();
  });

  it("stays manual under reduced motion", async () => {
    const media = stubMatchMedia(true);
    try {
      vi.useFakeTimers();
      renderSample();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(9000);
      });
      expect(screen.getByTestId("sample-phase-0")).toHaveAttribute(
        "data-active",
        "true",
      );
    } finally {
      media.restore();
    }
  });
});

describe("FirstUseSample source rack and page flow", () => {
  it("leads with the page, shows where it came from, then an AI answering from it", () => {
    renderSample();
    expect(screen.getByText("v2 launch plan")).toBeInTheDocument();
    expect(screen.getByTestId("sample-source-sample-memory")).toBeInTheDocument();
    expect(screen.getByTestId("sample-source-sample-file")).toBeInTheDocument();
    expect(screen.queryByTestId("sample-source-sample-page")).not.toBeInTheDocument();

    fireEvent.click(screen.getByTestId("sample-phase-1"));
    expect(screen.getByTestId("sample-source-sample-memory")).toHaveAttribute("data-lit", "true");
    expect(screen.getByTestId("sample-source-sample-file")).toHaveAttribute("data-dimmed", "true");

    fireEvent.click(screen.getByTestId("sample-phase-2"));
    expect(screen.getByTestId("sample-source-sample-file")).toHaveAttribute("data-lit", "true");
    expect(screen.getByTestId("sample-source-sample-memory")).toHaveAttribute("data-dimmed", "true");

    skipToResult();
    expect(screen.getByText("v2 launch plan")).toBeInTheDocument();
    expect(screen.queryByTestId("sample-source-sample-memory")).not.toBeInTheDocument();
    expect(screen.getByText("When does v2 launch, and what does it cost?")).toBeInTheDocument();
  });

  it("opens the file original with path and line context", () => {
    renderSample();
    skipToResult();
    fireEvent.click(screen.getByTestId("sample-citation-sample-cite-2"));
    const dialog = screen.getByTestId("sample-citation-dialog");
    expect(dialog).toHaveAttribute("data-source-type", "file");
    expect(
      within(dialog).getByText("File product folder / v2-meeting-notes.md · line 2"),
    ).toBeInTheDocument();
    // The quote appears twice: once as the cited excerpt, once highlighted
    // in the file original.
    expect(
      within(dialog).getAllByText("Launch stays on October 31."),
    ).toHaveLength(2);
  });

  it("opens the captured-memory original without file context", () => {
    renderSample();
    skipToResult();
    fireEvent.click(screen.getByTestId("sample-citation-sample-cite-1"));
    const dialog = screen.getByTestId("sample-citation-dialog");
    expect(dialog).toHaveAttribute("data-source-type", "memory");
    expect(
      within(dialog).getByText("Captured from conversation"),
    ).toBeInTheDocument();
    expect(
      within(dialog).queryByText(/File /),
    ).not.toBeInTheDocument();
  });

  it("reveals the AI step only through Commands for later, with a way back", () => {
    renderSample();
    skipToResult();
    expect(
      screen.queryByRole("heading", { name: "Commands for later" }),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Commands for later" }));
    expect(
      screen.getByRole("heading", { name: "Commands for later" }),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Back to the page" }));
    expect(
      screen.queryByRole("heading", { name: "Commands for later" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByText("v2 launch plan"),
    ).toBeInTheDocument();
  });
});

describe("FirstUseSample dialogs", () => {
  it("focuses the dialog on open, closes on Escape, and restores trigger focus", () => {
    renderSample();
    skipToResult();
    const trigger = screen.getByTestId("sample-citation-sample-cite-1");
    // Browsers focus a button on mousedown; jsdom needs the explicit focus.
    trigger.focus();
    fireEvent.click(trigger);
    const dialog = screen.getByTestId("sample-citation-dialog");
    expect(dialog.contains(document.activeElement)).toBe(true);
    expect(screen.getByTestId("first-use-sample")).toHaveAttribute("inert");
    fireEvent.keyDown(document, { key: "Escape" });
    expect(
      screen.queryByTestId("sample-citation-dialog"),
    ).not.toBeInTheDocument();
    expect(
      screen.getByTestId("first-use-sample"),
    ).not.toHaveAttribute("inert");
    expect(document.activeElement).toBe(trigger);
  });

  it("keeps Tab and Shift+Tab inside the dialog", () => {
    renderSample();
    skipToResult();
    fireEvent.click(screen.getByTestId("sample-citation-sample-cite-2"));
    const dialog = screen.getByTestId("sample-citation-dialog");
    const closeButton = within(dialog).getByRole("button", { name: "Close" });
    closeButton.focus();
    fireEvent.keyDown(document, { key: "Tab" });
    expect(document.activeElement).toBe(closeButton);
    fireEvent.keyDown(document, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(closeButton);
  });
});

describe("FirstUseSample AI-use panel", () => {
  it("suggests own-data recall: @wenlan for ChatGPT, slash commands otherwise", () => {
    renderSample();
    openAiPanel();
    fireEvent.click(screen.getByRole("tab", { name: "ChatGPT" }));
    expect(
      screen.getByText(/@wenlan recall the decisions/),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Codex" }));
    expect(
      screen.getByText("/recall decisions and next steps from my imports"),
    ).toBeInTheDocument();
    expect(screen.getByText("/handoff")).toBeInTheDocument();
    expect(screen.getByText("/brief")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Claude" }));
    expect(
      screen.getByText("/recall decisions and next steps from my imports"),
    ).toBeInTheDocument();
  });

  it("shows a read-only page instead of a handoff command for ChatGPT", () => {
    renderSample();
    openAiPanel();
    fireEvent.click(screen.getByRole("tab", { name: "ChatGPT" }));
    expect(
      screen.getByText(/ChatGPT can read but not write/),
    ).toBeInTheDocument();
    expect(screen.queryByText(/page today's decisions/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Codex" }));
    expect(screen.getByText("/handoff")).toBeInTheDocument();
  });

  it("reports a clipboard copy as copied", async () => {
    renderSample();
    openAiPanel();
    // The recall row lives in the tabpanel; the handoff/brief icon buttons
    // share the same accessible name outside it.
    const panel = screen.getByRole("tabpanel");
    fireEvent.click(
      within(panel).getByRole("button", { name: "Copy command" }),
    );
    expect(mockedClipboardWrite).toHaveBeenCalledTimes(1);
    expect(await within(panel).findByText("Copied")).toBeInTheDocument();
  });

  it("reports a clipboard rejection as failure, never as success", async () => {
    mockedClipboardWrite.mockRejectedValueOnce(new Error("denied"));
    renderSample();
    openAiPanel();
    const panel = screen.getByRole("tabpanel");
    fireEvent.click(
      within(panel).getByRole("button", { name: "Copy command" }),
    );
    expect(await screen.findByText(/Copy failed/)).toBeInTheDocument();
    expect(screen.queryByText("Copied")).not.toBeInTheDocument();
  });

  it("ignores stale copy resolutions that settle out of order", async () => {
    const resolvers: Array<() => void> = [];
    mockedClipboardWrite.mockImplementation(
      () => new Promise<void>((resolve) => resolvers.push(resolve)),
    );
    renderSample();
    openAiPanel();
    // Handoff only exists on writing-capable local clients (ChatGPT offers
    // none); the panel opens on Codex.
    const panel = screen.getByRole("tabpanel");
    fireEvent.click(
      within(panel).getByRole("button", { name: "Copy command" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Copy command: Save progress" }));
    await act(async () => {
      resolvers[1]!();
    });
    const handoffRow = screen.getByRole("button", { name: "Copy command: Save progress" }).closest("li")!;
    expect(within(handoffRow).getByTestId("copy-ok")).toBeInTheDocument();
    expect(within(handoffRow).getByRole("status")).toHaveTextContent("Save progress: Copied");
    await act(async () => {
      resolvers[0]!();
    });
    // The stale recall resolution changes nothing: no recall success, no error.
    expect(within(panel).queryByTestId("copy-ok")).not.toBeInTheDocument();
    expect(screen.queryByText(/Copy failed/)).not.toBeInTheDocument();
    expect(within(handoffRow).getByTestId("copy-ok")).toBeInTheDocument();
  });

  it("drops a pending copy when the client switches before it settles", async () => {
    const resolvers: Array<() => void> = [];
    mockedClipboardWrite.mockImplementation(
      () => new Promise<void>((resolve) => resolvers.push(resolve)),
    );
    renderSample();
    openAiPanel();
    const panel = screen.getByRole("tabpanel");
    fireEvent.click(
      within(panel).getByRole("button", { name: "Copy command" }),
    );
    fireEvent.click(screen.getByRole("tab", { name: "Codex" }));
    await act(async () => {
      resolvers[0]!();
    });
    expect(mockedClipboardWrite).toHaveBeenCalledTimes(1);
    expect(screen.queryByText("Copied")).not.toBeInTheDocument();
    expect(screen.queryByText(/Copy failed/)).not.toBeInTheDocument();
  });

  it("ends the AI panel with exactly one primary CTA that calls onFinish, and no connect or import CTAs", () => {
    const onFinish = vi.fn();
    const onBack = vi.fn();
    renderSample({ onFinish, onBack });
    openAiPanel();
    expect(screen.queryByRole("button", { name: "Connect my AI" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Bring my data" })).not.toBeInTheDocument();
    const panel = screen.getByRole("tab", { name: "ChatGPT" }).closest("section")!;
    const primary = panel.querySelectorAll("button.fug-button-primary");
    expect(primary).toHaveLength(1);
    expect(primary[0]).toHaveTextContent("Back to home");
    expect(screen.getByText(i18n.t("firstUse.sample.notInLibrary"))).toBeVisible();
    fireEvent.click(primary[0]!);
    expect(onFinish).toHaveBeenCalledTimes(1);
    expect(onBack).not.toHaveBeenCalled();
  });

  it("uses the header back button for onBack and honours custom labels", () => {
    const onBack = vi.fn();
    renderSample({ onBack, backLabel: "Custom back", finishLabel: "Custom finish" });
    fireEvent.click(screen.getByRole("button", { name: "Custom back" }));
    expect(onBack).toHaveBeenCalledTimes(1);
    openAiPanel();
    expect(screen.getByRole("button", { name: "Custom finish" })).toBeInTheDocument();
  });

  it("explains read-only ChatGPT without a handoff command but keeps writes for local clients", () => {
    renderSample();
    openAiPanel();
    fireEvent.click(screen.getByRole("tab", { name: "ChatGPT" }));
    // ChatGPT: explain the limitation, never offer a save action.
    expect(
      screen.getByText(/ChatGPT can read but not write/),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Copy command: Save progress" })).not.toBeInTheDocument();
    expect(screen.queryByText("/handoff")).not.toBeInTheDocument();
    expect(
      screen.getByText(/@wenlan recall the decisions/),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Codex" }));
    expect(screen.getByText(/Save progress/)).toBeInTheDocument();
    expect(screen.getByText("/handoff")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Claude" }));
    expect(screen.getByText(/Save progress/)).toBeInTheDocument();
  });

  it("shows the ChatGPT read-only page only for ChatGPT", () => {
    renderSample();
    openAiPanel();
    fireEvent.click(screen.getByRole("tab", { name: "ChatGPT" }));
    expect(
      screen.getByText(/through the web relay/),
    ).toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Codex" }));
    expect(
      screen.queryByText(/through the web relay/),
    ).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Claude" }));
    expect(
      screen.queryByText(/through the web relay/),
    ).not.toBeInTheDocument();
  });

  it("announces which secondary command was copied and reports its failure", async () => {
    renderSample();
    openAiPanel();
    const copy = screen.getByRole("button", { name: "Copy command: Brief a new chat" });
    fireEvent.click(copy);
    expect(await within(copy.closest("li")!).findByText("Brief a new chat: Copied")).toHaveAttribute("role", "status");
    mockedClipboardWrite.mockRejectedValueOnce(new Error("denied"));
    fireEvent.click(copy);
    expect(await within(copy.closest("li")!).findByRole("alert")).toHaveTextContent("Copy failed");
    expect(within(copy.closest("li")!).getByRole("status")).toBeEmptyDOMElement();
  });

  it("ships a translated ChatGPT disclosure in all three locales", async () => {
    const { enFirstUse, hansFirstUse, hantFirstUse } = await import(
      "../firstUseCopy"
    );
    for (const copy of [enFirstUse, hansFirstUse, hantFirstUse]) {
      expect(copy.sample.chatgptNote).toBeTruthy();
    }
    expect(enFirstUse.sample.chatgptNote).toMatch(/read-only/);
    expect(enFirstUse.sample.chatgptNote).toMatch(/running and connected on your computer/);
    expect(enFirstUse.sample.chatgptNote).toMatch(/relay/);
    expect(hansFirstUse.sample.chatgptNote).toMatch(/只读/);
    expect(hansFirstUse.sample.chatgptNote).toMatch(/运行和联网/);
    expect(hansFirstUse.sample.chatgptNote).toMatch(/转发服务/);
    expect(hantFirstUse.sample.chatgptNote).toMatch(/唯讀/);
    expect(hantFirstUse.sample.chatgptNote).toMatch(/運作與連線/);
    expect(hantFirstUse.sample.chatgptNote).toMatch(/轉送服務/);
    // No locale promises that data stays on the machine.
    for (const copy of [enFirstUse, hansFirstUse, hantFirstUse]) {
      expect(copy.sample.chatgptNote).not.toMatch(/never leaves|no data leaves/i);
      expect(copy.sample.chatgptNote).not.toMatch(/不[会會]离开|不會離開/);
    }
  });

  it.each(["en", "zh-Hant", "zh-Hans"])("renders capability-aware setup in %s", async (locale) => {
    await i18n.changeLanguage(locale);
    const onFinish = vi.fn();
    renderSample({ onFinish });
    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.sample.skipToResult") }));
    fireEvent.click(screen.getByRole("button", { name: i18n.t("firstUse.sample.useWithAi") }));
    // Opens on Codex, the tool the sample chat used.
    expect(screen.getByRole("tab", { name: "Codex" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByText(i18n.t("firstUse.sample.chatgptNote"))).not.toBeInTheDocument();
    expect(screen.getByText("/handoff")).toBeVisible();
    expect(screen.getByText(i18n.t("firstUse.sample.useBody"))).toBeVisible();
    // Header back and the final button share the label; the final one is last.
    const finishButtons = screen.getAllByRole("button", { name: i18n.t("firstUse.sample.finish") });
    fireEvent.click(finishButtons[finishButtons.length - 1]!);
    expect(onFinish).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("tab", { name: "ChatGPT" }));
    expect(screen.getByText(i18n.t("firstUse.sample.chatgptNote"))).toBeVisible();
    expect(screen.queryByText("/handoff")).not.toBeInTheDocument();
    expect(mockedInvoke).not.toHaveBeenCalled();
  });
});

describe("FirstUseSample reduced motion", () => {
  it("offers no Play control but keeps manual steps working", () => {
    const media = stubMatchMedia(true);
    try {
      renderSample();
      expect(
        screen.queryByRole("button", { name: "Play" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "Pause" }),
      ).not.toBeInTheDocument();
      fireEvent.click(screen.getByRole("button", { name: "Next step" }));
      expect(screen.getByTestId("sample-phase-1")).toHaveAttribute(
        "data-active",
        "true",
      );
      fireEvent.click(screen.getByRole("button", { name: "Replay" }));
      expect(screen.getByTestId("sample-phase-0")).toHaveAttribute(
        "data-active",
        "true",
      );
    } finally {
      media.restore();
    }
  });

  it("hides Play on the final phase where replay suffices", () => {
    renderSample();
    skipToResult();
    expect(
      screen.queryByRole("button", { name: "Play" }),
    ).not.toBeInTheDocument();
    expect(
      screen.getByRole("button", { name: "Replay" }),
    ).toBeInTheDocument();
  });

  it("follows media preference changes and cleans up its listener", () => {
    const media = stubMatchMedia(false);
    try {
      const { unmount } = renderSample();
      expect(
        screen.getByRole("button", { name: "Pause" }),
      ).toBeInTheDocument();
      act(() => {
        media.setMatches(true);
      });
      expect(
        screen.queryByRole("button", { name: "Pause" }),
      ).not.toBeInTheDocument();
      expect(
        screen.queryByRole("button", { name: "Play" }),
      ).not.toBeInTheDocument();
      unmount();
      expect(media.mq.removeEventListener).toHaveBeenCalledWith(
        "change",
        expect.any(Function),
      );
    } finally {
      media.restore();
    }
  });
});
