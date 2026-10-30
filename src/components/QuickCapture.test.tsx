// SPDX-License-Identifier: AGPL-3.0-only
import { invoke } from "@tauri-apps/api/core";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import QuickCapture from "./QuickCapture";
vi.mock("./memory/editor/MarkdownEditor", async () => ({ MarkdownEditor: (await import("./memory/editor/NativeMarkdownEditor")).NativeMarkdownEditor }));

const mockedInvoke = vi.mocked(invoke);
beforeEach(() => { localStorage.clear(); });

// The standalone window draws no exterior shadow on any platform, so the user
// agents below only prove the absence holds everywhere rather than scoping it.
const MACOS_WKWEBVIEW =
  "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.4 Safari/605.1.15";
const WINDOWS_WEBVIEW2 =
  "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0";
const LINUX_WEBKITGTK =
  "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15";

const originalNavigator = Object.getOwnPropertyDescriptor(globalThis, "navigator");

/** Replace the ambient user agent for a render. */
function setPlatform(userAgent: string | undefined): void {
  Object.defineProperty(globalThis, "navigator", {
    value: userAgent === undefined ? undefined : { userAgent },
    configurable: true,
  });
}

afterEach(() => {
  if (originalNavigator) {
    Object.defineProperty(globalThis, "navigator", originalNavigator);
  } else {
    Reflect.deleteProperty(globalThis, "navigator");
  }
});

/** The card, selected by its stable test id. */
function findCard(container: HTMLElement): HTMLElement {
  const tagged = container.querySelectorAll('[data-testid="quick-capture-card"]');
  expect(tagged).toHaveLength(1);
  return tagged[0] as HTMLElement;
}

/**
 * Renders and hands back the two elements the platform scoping touches: the
 * standalone window's outer wrapper (which owns the transparent inset) and the
 * card itself (which owns the shadow).
 */
function renderCapture(standalone: boolean) {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false }, mutations: { retry: false } },
  });
  const { container } = render(
    <QueryClientProvider client={client}>
      <QuickCapture isOpen onClose={() => {}} standalone={standalone} />
    </QueryClientProvider>,
  );
  const wrapper = container.firstElementChild as HTMLElement;
  expect(wrapper).toBeTruthy();
  const card = findCard(container);
  // The card is INSIDE the wrapper whose inset is asserted alongside it. Both
  // halves of every test below are then known to be about one window rather
  // than about two elements that happen to be in the same container.
  expect(wrapper.contains(card)).toBe(true);
  expect(wrapper).not.toBe(card);
  return { container, wrapper, card };
}

/** Inset in px, whichever way React and jsdom chose to serialise a zero. */
function insetPx(el: HTMLElement): number {
  return parseInt(el.style.padding || "0", 10);
}

function queryTextareaAndSave(container: HTMLElement) {
  const textarea = container.querySelector("textarea");
  expect(textarea).toBeTruthy();
  const buttons = Array.from(container.querySelectorAll("button"));
  const saveButton = buttons.find((b) => b.textContent === "Save");
  expect(saveButton).toBeTruthy();
  return { textarea: textarea as HTMLTextAreaElement, saveButton: saveButton as HTMLButtonElement };
}

describe("QuickCapture standalone window has no exterior shadow or inset", () => {
  const platforms: Array<[string, string | undefined]> = [
    ["macOS", MACOS_WKWEBVIEW],
    ["Windows", WINDOWS_WEBVIEW2],
    ["Linux", LINUX_WEBKITGTK],
    ["unknown", undefined],
  ];

  it.each(platforms)("empty draft on %s: no shadow, no inset", (_name, ua) => {
    setPlatform(ua);
    const { wrapper, card } = renderCapture(true);

    expect(card.style.boxShadow).toBe("none");
    expect(insetPx(wrapper)).toBe(0);
  });

  it.each(platforms)("draft with content on %s: no shadow or glow, no inset", (_name, ua) => {
    setPlatform(ua);
    const { container, wrapper } = renderCapture(true);

    const { textarea } = queryTextareaAndSave(container);
    fireEvent.change(textarea, { target: { value: "long enough to save this draft" } });

    const freshCard = findCard(container);
    expect(freshCard.style.boxShadow).toBe("none");
    expect(insetPx(wrapper)).toBe(0);
  });

  it.each(platforms)("pending save on %s: no shadow or glow, no inset", async (_name, ua) => {
    setPlatform(ua);
    mockedInvoke.mockReset();
    // Never resolves: the mutation stays pending for the assertion.
    mockedInvoke.mockImplementationOnce(() => new Promise(() => {}));
    const { container, wrapper } = renderCapture(true);

    const { textarea, saveButton } = queryTextareaAndSave(container);
    fireEvent.change(textarea, { target: { value: "long enough to save this draft" } });
    fireEvent.click(saveButton);

    // Pending label proves the mutation started; the card must still be flat.
    expect(await screen.findByText("Saving...")).toBeTruthy();
    const freshCard = findCard(container);
    expect(freshCard.style.boxShadow).toBe("none");
    expect(insetPx(wrapper)).toBe(0);
  });

  it.each(platforms)("saved state on %s: no shadow or glow, no inset", async (_name, ua) => {
    setPlatform(ua);
    mockedInvoke.mockReset();
    mockedInvoke.mockResolvedValueOnce(1);
    const { container, wrapper } = renderCapture(true);

    const { textarea, saveButton } = queryTextareaAndSave(container);
    fireEvent.change(textarea, { target: { value: "long enough to save this draft" } });
    fireEvent.click(saveButton);

    expect(await screen.findByText("Saved to memory")).toBeTruthy();
    const freshCard = findCard(container);
    expect(freshCard.style.boxShadow).toBe("none");
    expect(insetPx(wrapper)).toBe(0);
  });

  it("leaves the modal's shadow alone on every platform", () => {
    // The modal floats over the app on an opaque backdrop with room around it.
    for (const ua of [MACOS_WKWEBVIEW, WINDOWS_WEBVIEW2, LINUX_WEBKITGTK]) {
      setPlatform(ua);
      const { card } = renderCapture(false);
      expect(card.style.boxShadow).not.toBe("none");
      expect(card.style.boxShadow).toContain("32px");
    }
  });

  it("keeps the modal's draft glow", () => {
    setPlatform(MACOS_WKWEBVIEW);
    const { container } = renderCapture(false);

    const { textarea } = queryTextareaAndSave(container);
    fireEvent.change(textarea, { target: { value: "long enough to save this draft" } });

    const freshCard = findCard(container);
    expect(freshCard.style.boxShadow).not.toBe("none");
    expect(freshCard.style.boxShadow).toContain("32px");
  });
});

describe("QuickCapture surfaces a save error instead of swallowing it", () => {
  beforeEach(() => {
    mockedInvoke.mockReset();
  });

  it("shows an inline localized error and keeps the draft when the capture invoke rejects", async () => {
    mockedInvoke.mockRejectedValueOnce(
      new Error("Memory content must be at least 10 characters"),
    );
    renderCapture(false);

    const textarea = screen.getByRole("textbox", { name: "Quick capture" });
    fireEvent.change(textarea, { target: { value: "long enough to save" } });
    fireEvent.click(screen.getByText("Save"));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe(
      "Couldn't save: Memory content must be at least 10 characters",
    );
    // The draft is not cleared on a rejected capture -- only onSuccess clears it.
    expect((textarea as HTMLTextAreaElement).value).toBe("long enough to save");
  });

  it("blocks a too-short draft client-side without invoking", async () => {
    renderCapture(false);

    const textarea = screen.getByRole("textbox", { name: "Quick capture" });
    fireEvent.change(textarea, { target: { value: "今天天氣非常晴朗好" } });
    fireEvent.click(screen.getByText("Save"));

    const alert = await screen.findByRole("alert");
    expect(alert.textContent).toBe("Write at least 10 characters before saving.");
    expect(mockedInvoke).not.toHaveBeenCalled();
    expect((textarea as HTMLTextAreaElement).value).toBe("今天天氣非常晴朗好");
  });
});
