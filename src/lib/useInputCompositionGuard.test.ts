import { act, fireEvent, renderHook, waitFor } from "@testing-library/react";
import { expect, it, vi } from "vitest";
const setEditorComposing = vi.hoisted(() => vi.fn());
vi.mock("./tauri", () => ({ setEditorComposing }));
import { useInputCompositionGuard } from "./useInputCompositionGuard";

it("serializes native reports so an old end cannot overtake a new composition", async () => {
  const releases: Array<() => void> = [];
  setEditorComposing.mockImplementation(() => new Promise<void>(resolve => releases.push(resolve)));
  const input = document.createElement("input"); document.body.append(input);
  const hook = renderHook(() => useInputCompositionGuard());
  await waitFor(() => expect(setEditorComposing.mock.calls).toEqual([[false]]));
  fireEvent.compositionStart(input);
  fireEvent.compositionEnd(input, { data: "你" });
  fireEvent.compositionStart(input);
  expect(setEditorComposing.mock.calls).toEqual([[false]]);
  for (let calls = 2; calls <= 4; calls++) {
    await act(async () => { releases.shift()!(); await Promise.resolve(); });
    await waitFor(() => expect(setEditorComposing).toHaveBeenCalledTimes(calls));
  }
  expect(setEditorComposing.mock.calls).toEqual([[false], [true], [false], [true]]);
  hook.unmount();
  await act(async () => { releases.shift()!(); await Promise.resolve(); });
  await waitFor(() => expect(setEditorComposing).toHaveBeenLastCalledWith(false));
  await act(async () => { releases.shift()!(); await Promise.resolve(); });
  input.remove();
});
