// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { quickCapture } from "../lib/tauri";
import { setConfirmPresenter } from "../lib/confirmAction";
import { quickCaptureDraftKey } from "./quickCaptureDraftRecovery";
import QuickCapture from "./QuickCapture";
vi.mock("./memory/editor/MarkdownEditor", async () => ({ MarkdownEditor: (await import("./memory/editor/NativeMarkdownEditor")).NativeMarkdownEditor }));
vi.mock("./memory/editor/useEditorLinkNotes", () => ({ useEditorLinkNotes: () => [] }));
vi.mock("../lib/tauri", () => ({ quickCapture: vi.fn() }));
const body = "  中文 test note with spaces.  \n";
const key = quickCaptureDraftKey(true);
beforeEach(() => { vi.mocked(quickCapture).mockReset(); localStorage.clear(); setConfirmPresenter(async () => true); });
afterEach(() => { vi.restoreAllMocks(); setConfirmPresenter(); });
function mount(close = vi.fn()) {
  const view = render(<QueryClientProvider client={new QueryClient({ defaultOptions: { mutations: { retry: false } } })}><QuickCapture isOpen standalone onClose={close} /></QueryClientProvider>);
  return { ...view, close, editor: screen.getByRole("textbox", { name: "Quick capture" }) };
}
function type(editor: HTMLElement, value = body) { fireEvent.change(editor, { target: { value } }); }
function save(editor: HTMLElement) { fireEvent.keyDown(editor, { key: "s", ctrlKey: true }); }
it("checkpoints exact input synchronously and recovers after unmount/remount", () => {
  const first = mount(); type(first.editor);
  expect(localStorage.getItem(key)).toBe(body);
  first.unmount();
  expect(mount().editor).toHaveValue(body);
});
it("keeps composition local, submits exact body once, and refuses close while saving", async () => {
  let reject!: (error: Error) => void;
  vi.mocked(quickCapture).mockReturnValue(new Promise((_resolve, rejectPromise) => { reject = rejectPromise; }));
  const { editor, close } = mount();
  fireEvent.compositionStart(editor); type(editor);
  fireEvent.keyDown(editor, { key: "Enter", ctrlKey: true }); fireEvent.keyDown(editor, { key: "Escape" });
  expect(quickCapture).not.toHaveBeenCalled(); expect(close).not.toHaveBeenCalled();
  expect(localStorage.getItem(key)).toBe(body);
  fireEvent.compositionEnd(editor); save(editor); fireEvent.keyDown(editor, { key: "Enter", ctrlKey: true });
  await waitFor(() => expect(quickCapture).toHaveBeenCalledTimes(1));
  expect(vi.mocked(quickCapture).mock.calls[0][0]).toEqual({ content: body });
  fireEvent.keyDown(editor, { key: "Escape" }); expect(close).not.toHaveBeenCalled();
  await act(async () => { reject(new Error("offline")); });
  expect(await screen.findByRole("alert")).toHaveTextContent("offline"); expect(editor).toHaveValue(body);
  expect(localStorage.getItem(key)).toBe(body);
});
it("clears the checkpoint only after a successful canonical write", async () => {
  vi.mocked(quickCapture).mockResolvedValue(1);
  const view = mount(); type(view.editor); save(view.editor);
  await screen.findByText("Saved to memory");
  expect(localStorage.getItem(key)).toBeNull(); view.unmount();
  expect(mount().editor).toHaveValue("");
});
it("keeps rejected discard, then clears both the accepted discard and checkpoint", async () => {
  const view = mount(); type(view.editor);
  setConfirmPresenter(async () => false);
  fireEvent.keyDown(view.editor, { key: "Escape" });
  await act(async () => {});
  expect(view.close).not.toHaveBeenCalled(); expect(localStorage.getItem(key)).toBe(body);
  setConfirmPresenter(async () => true);
  fireEvent.keyDown(view.editor, { key: "Escape" });
  await waitFor(() => expect(view.close).toHaveBeenCalledTimes(1));
  expect(localStorage.getItem(key)).toBeNull(); view.unmount();
  expect(mount().editor).toHaveValue("");
});
it("treats zero chunks as failure and retains the recoverable draft", async () => {
  vi.mocked(quickCapture).mockResolvedValue(0);
  const view = mount(); type(view.editor); save(view.editor);
  await screen.findByRole("alert");
  expect(screen.queryByText("Saved to memory")).not.toBeInTheDocument();
  expect(view.editor).toHaveValue(body); expect(localStorage.getItem(key)).toBe(body);
  expect(view.close).not.toHaveBeenCalled();
});
it("shows failed local storage access without dropping the session text", () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
  const { editor } = mount(); expect(screen.getByRole("alert")).toBeVisible();
  type(editor); expect(editor).toHaveValue(body); expect(screen.getByRole("alert")).toBeVisible();
});
it("keeps the draft and explains a failed discard cleanup", async () => {
  const view = mount(); type(view.editor);
  vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => { throw new Error("blocked"); });
  fireEvent.keyDown(view.editor, { key: "Escape" });
  await screen.findByRole("alert");
  expect(view.editor).toHaveValue(body); expect(view.close).not.toHaveBeenCalled();
  expect(localStorage.getItem(key)).toBe(body);
});
it("retains visible text if successful save cannot clear local recovery", async () => {
  vi.mocked(quickCapture).mockResolvedValue(1);
  const view = mount(); type(view.editor);
  vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => { throw new Error("blocked"); });
  save(view.editor); await screen.findByText("Saved to memory"); await screen.findByRole("alert");
  expect(view.editor).toHaveValue(body); expect(localStorage.getItem(key)).toBe(body);
  expect(view.close).not.toHaveBeenCalled();
});
it("keeps the ten Unicode character gate and separate modal recovery slot", () => {
  localStorage.setItem(quickCaptureDraftKey(false), "modal draft");
  const view = mount(); expect(view.editor).toHaveValue("");
  type(view.editor, "😀".repeat(9)); save(view.editor);
  expect(quickCapture).not.toHaveBeenCalled();
  expect(screen.getByRole("alert")).toBeVisible();
  expect(localStorage.getItem(quickCaptureDraftKey(false))).toBe("modal draft");
});
