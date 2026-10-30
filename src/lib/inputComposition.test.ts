// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent } from "@testing-library/react";
import {
  COMPOSITION_HEARTBEAT_MS,
  COMPOSITION_IDLE_RESET_MS,
  commitInputComposition,
  isInputCompositionActive,
  observeInputComposition,
} from "./inputComposition";

const cleanups: Array<() => void> = [];
afterEach(() => {
  for (const cleanup of cleanups.splice(0)) cleanup();
  document.body.replaceChildren();
  vi.useRealTimers();
  vi.restoreAllMocks();
});
function setup(tag: "input" | "textarea" | "div" = "input") {
  const input = document.createElement(tag);
  if (tag === "div") input.contentEditable = "true";
  document.body.append(input);
  const report = vi.fn();
  cleanups.push(observeInputComposition(report));
  return { input, report };
}

describe("application IME navigation guard", () => {
  it.each(["input", "textarea", "div"] as const)("tracks the real lifecycle for %s", tag => {
    const { input, report } = setup(tag);
    fireEvent.compositionStart(input);
    expect(isInputCompositionActive()).toBe(true);
    fireEvent.compositionUpdate(input, { data: "ni" });
    expect(isInputCompositionActive()).toBe(true);
    fireEvent.compositionEnd(input, { data: "你好" });
    expect(isInputCompositionActive()).toBe(false);
    expect(report.mock.calls).toEqual([[false], [true], [false]]);
  });

  it("clears native state when a composing field is removed", async () => {
    const { input, report } = setup();
    fireEvent.compositionStart(input);
    input.remove();
    await Promise.resolve();
    expect(isInputCompositionActive()).toBe(false);
    expect(report).toHaveBeenLastCalledWith(false);
  });

  it("does not clear a newer field when an old field ends", () => {
    const { input } = setup();
    const other = document.createElement("input"); document.body.append(other);
    fireEvent.compositionStart(input);
    fireEvent.compositionStart(other);
    fireEvent.compositionEnd(input);
    expect(isInputCompositionActive()).toBe(true);
    fireEvent.compositionEnd(other);
    expect(isInputCompositionActive()).toBe(false);
  });

  it("releases on pagehide and unmount", () => {
    const { input, report } = setup();
    fireEvent.compositionStart(input);
    fireEvent(window, new Event("pagehide"));
    expect(isInputCompositionActive()).toBe(false);
    fireEvent.compositionStart(input);
    cleanups.pop()!();
    expect(isInputCompositionActive()).toBe(false);
    expect(report).toHaveBeenLastCalledWith(false);
  });
});

describe("a composition whose compositionend never arrives", () => {
  it("is released when the window loses focus (Alt+Tab mid-composition)", () => {
    const { input, report } = setup();
    fireEvent.compositionStart(input);
    fireEvent(window, new Event("blur"));
    expect(isInputCompositionActive()).toBe(false);
    expect(report.mock.calls).toEqual([[false], [true], [false]]);
    // A late end for the released field changes nothing.
    fireEvent.compositionEnd(input);
    expect(report).toHaveBeenCalledTimes(3);
  });

  it("blocks again when a released composition turns out to be alive", () => {
    const { input, report } = setup();
    fireEvent.compositionStart(input);
    fireEvent(window, new Event("blur"));
    fireEvent.compositionUpdate(input, { data: "ㄋㄧˇ" });
    expect(isInputCompositionActive()).toBe(true);
    fireEvent.compositionEnd(input, { data: "你" });
    expect(isInputCompositionActive()).toBe(false);
    expect(report.mock.calls).toEqual([[false], [true], [false], [true], [false]]);
  });

  it("is released after the idle bound, with a log line", () => {
    vi.useFakeTimers();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const { input, report } = setup();
    fireEvent.compositionStart(input);
    vi.advanceTimersByTime(COMPOSITION_IDLE_RESET_MS - 1);
    expect(isInputCompositionActive()).toBe(true);
    vi.advanceTimersByTime(1);
    expect(isInputCompositionActive()).toBe(false);
    expect(report).toHaveBeenLastCalledWith(false);
    expect(warn).toHaveBeenCalledWith(expect.stringContaining("without compositionend"));
  });

  it("keeps a long composition alive while it is typed into, re-reporting it for the native bound", () => {
    vi.useFakeTimers();
    const { input, report } = setup();
    fireEvent.compositionStart(input);
    for (let elapsed = 0; elapsed < COMPOSITION_IDLE_RESET_MS * 2; elapsed += 5_000) {
      vi.advanceTimersByTime(5_000);
      fireEvent.compositionUpdate(input, { data: "ㄅ" });
    }
    expect(isInputCompositionActive()).toBe(true);
    const heartbeats = report.mock.calls.filter(([active]) => active).length - 1;
    expect(heartbeats).toBe(Math.floor((COMPOSITION_IDLE_RESET_MS * 2) / COMPOSITION_HEARTBEAT_MS));
    expect(report.mock.calls.slice(1).every(([active]) => active)).toBe(true);
  });
});

describe("commitInputComposition", () => {
  it("resolves at once when nothing is composing", async () => {
    setup();
    await expect(commitInputComposition()).resolves.toBe(true);
  });

  it("blurs the composing field so the browser commits the preedit before a flush", async () => {
    const { input } = setup("textarea");
    // jsdom has no IME; this is what Chromium and WebKit do on blur.
    input.addEventListener("blur", () => fireEvent.compositionEnd(input, { data: "你好" }));
    input.focus();
    fireEvent.compositionStart(input);
    await expect(commitInputComposition()).resolves.toBe(true);
    expect(isInputCompositionActive()).toBe(false);
    expect(document.activeElement).not.toBe(input);
  });

  it("treats the preedit as text when the browser never ends the composition", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    const { input, report } = setup();
    input.focus();
    fireEvent.compositionStart(input);
    await expect(commitInputComposition(document, 20)).resolves.toBe(false);
    expect(isInputCompositionActive()).toBe(false);
    expect(report).toHaveBeenLastCalledWith(false);
  });
});
