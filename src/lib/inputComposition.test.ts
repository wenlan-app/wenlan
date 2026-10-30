import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent } from "@testing-library/react";
import { isInputCompositionActive, observeInputComposition } from "./inputComposition";

const cleanups: Array<() => void> = [];
afterEach(() => { for (const cleanup of cleanups.splice(0)) cleanup(); document.body.replaceChildren(); });
function setup(tag: "input" | "textarea" | "div" = "input") {
  const input = document.createElement(tag);
  if (tag === "div") input.contentEditable = "true";
  document.body.append(input);
  const report = vi.fn();
  cleanups.push(observeInputComposition(report));
  return { input, report };
}

describe("application IME navigation guard", () => {
  it.each(["input", "textarea", "div"] as const)("tracks real lifecycle for %s without relying on window focus", tag => {
    const { input, report } = setup(tag);
    fireEvent.compositionStart(input);
    expect(isInputCompositionActive()).toBe(true);
    fireEvent(window, new Event("blur"));
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

  it("releases on pagehide and unmount without timing guesses", () => {
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
