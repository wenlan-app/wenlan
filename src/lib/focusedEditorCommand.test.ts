import { afterEach, expect, it, vi } from "vitest";
import { fireEvent } from "@testing-library/react";
import { requestFocusedMarkdownLink } from "./focusedEditorCommand";
import { observeInputComposition } from "./inputComposition";
let stop: (() => void) | undefined;
afterEach(() => { stop?.(); stop = undefined; document.body.replaceChildren(); });

it("consumes the native primary shortcut while a title input composes", () => {
  const input = document.createElement("input"); document.body.append(input); input.focus();
  stop = observeInputComposition(vi.fn());
  fireEvent.compositionStart(input);
  expect(requestFocusedMarkdownLink()).toBe(true);
  expect(input).toHaveFocus();
  fireEvent.compositionEnd(input, { data: "你好" });
  expect(requestFocusedMarkdownLink()).toBe(false);
});
