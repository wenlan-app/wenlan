// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { quickCapture } from "../lib/tauri";
import { setConfirmPresenter } from "../lib/confirmAction";
import { quickCaptureDraftKey } from "./quickCaptureDraftRecovery";
import QuickCapture from "./QuickCapture";

// The plain-text fallback stands in for the lazily loaded CodeMirror engine.
vi.mock("./memory/editor/MarkdownEditor", async () => ({
  MarkdownEditor: (await import("./memory/editor/NativeMarkdownEditor")).NativeMarkdownEditor,
}));
vi.mock("./memory/editor/useEditorLinkNotes", () => ({ useEditorLinkNotes: () => [] }));
vi.mock("../lib/tauri", () => ({ quickCapture: vi.fn() }));

const body = "  中文 test note with spaces.  \n";
const key = quickCaptureDraftKey(true);
const confirmQuestions: string[] = [];

function answerConfirm(answer: boolean) {
  setConfirmPresenter(async (message) => {
    confirmQuestions.push(message);
    return answer;
  });
}

beforeEach(() => {
  vi.mocked(quickCapture).mockReset();
  localStorage.clear();
  confirmQuestions.length = 0;
  answerConfirm(true);
});
afterEach(() => {
  vi.restoreAllMocks();
  setConfirmPresenter();
});

function mount(close = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { mutations: { retry: false } } });
  const view = render(
    <QueryClientProvider client={client}>
      <QuickCapture isOpen standalone onClose={close} />
    </QueryClientProvider>,
  );
  return { ...view, close, editor: screen.getByRole("textbox", { name: "Quick capture" }) };
}
function type(editor: HTMLElement, value = body) {
  fireEvent.change(editor, { target: { value } });
}
function save(editor: HTMLElement) {
  fireEvent.keyDown(editor, { key: "Enter", ctrlKey: true });
}

it("keeps a copy of the exact text on this device and restores it after a remount", () => {
  const first = mount();
  type(first.editor);
  expect(localStorage.getItem(key)).toBe(body);
  first.unmount();
  expect(mount().editor).toHaveValue(body);
});

it("ignores keys while the IME composes, sends the exact body once, and refuses to close while saving", async () => {
  let reject!: (error: Error) => void;
  vi.mocked(quickCapture).mockReturnValue(new Promise((_resolve, rejectPromise) => { reject = rejectPromise; }));
  const { editor, close } = mount();
  fireEvent.compositionStart(editor);
  type(editor);
  fireEvent.keyDown(editor, { key: "Enter", ctrlKey: true });
  fireEvent.keyDown(editor, { key: "Escape" });
  expect(quickCapture).not.toHaveBeenCalled();
  expect(close).not.toHaveBeenCalled();
  expect(localStorage.getItem(key)).toBe(body);

  fireEvent.compositionEnd(editor);
  save(editor);
  save(editor);
  await waitFor(() => expect(quickCapture).toHaveBeenCalledTimes(1));
  expect(vi.mocked(quickCapture).mock.calls[0][0]).toEqual({ content: body });
  fireEvent.keyDown(editor, { key: "Escape" });
  expect(close).not.toHaveBeenCalled();

  await act(async () => { reject(new Error("offline")); });
  const alert = await screen.findByRole("alert");
  expect(alert).toHaveTextContent("Couldn't save to memory: offline");
  expect(alert).toHaveTextContent("Your text is kept on this device.");
  expect(editor).toHaveValue(body);
  expect(localStorage.getItem(key)).toBe(body);
});

it("leaves Escape and Cmd/Ctrl+Enter outside the editor to the IME while it composes", async () => {
  vi.mocked(quickCapture).mockResolvedValue(1);
  const { editor, close } = mount();
  type(editor);
  const closeButton = screen.getByTitle("Close (Esc)");
  fireEvent.keyDown(closeButton, { key: "Escape", isComposing: true });
  fireEvent.keyDown(closeButton, { key: "Enter", ctrlKey: true, keyCode: 229 });
  await act(async () => {});
  expect(confirmQuestions).toEqual([]);
  expect(close).not.toHaveBeenCalled();
  expect(quickCapture).not.toHaveBeenCalled();

  fireEvent.keyDown(closeButton, { key: "Enter", ctrlKey: true });
  await waitFor(() => expect(quickCapture).toHaveBeenCalledTimes(1));
});

it("clears the device copy only after the memory is saved", async () => {
  vi.mocked(quickCapture).mockResolvedValue(1);
  const view = mount();
  type(view.editor);
  save(view.editor);
  await screen.findByText("Saved to memory");
  expect(localStorage.getItem(key)).toBeNull();
  await waitFor(() => expect(view.close).toHaveBeenCalledTimes(1));
  view.unmount();
  expect(mount().editor).toHaveValue("");
});

it("asks with its own wording before discarding, keeps the text on no, and clears it on yes", async () => {
  const view = mount();
  type(view.editor);
  answerConfirm(false);
  fireEvent.keyDown(view.editor, { key: "Escape" });
  await act(async () => {});
  expect(confirmQuestions).toEqual(["Discard this text? It hasn't been saved to memory."]);
  expect(view.close).not.toHaveBeenCalled();
  expect(localStorage.getItem(key)).toBe(body);

  answerConfirm(true);
  fireEvent.keyDown(view.editor, { key: "Escape" });
  await waitFor(() => expect(view.close).toHaveBeenCalledTimes(1));
  expect(localStorage.getItem(key)).toBeNull();
  view.unmount();
  expect(mount().editor).toHaveValue("");
});

it("closes an empty window without asking", async () => {
  const view = mount();
  fireEvent.keyDown(view.editor, { key: "Escape" });
  await waitFor(() => expect(view.close).toHaveBeenCalledTimes(1));
  expect(confirmQuestions).toEqual([]);
});

it("treats zero saved chunks as a failure and keeps the text", async () => {
  vi.mocked(quickCapture).mockResolvedValue(0);
  const view = mount();
  type(view.editor);
  save(view.editor);
  expect(await screen.findByRole("alert")).toHaveTextContent("Nothing was saved to memory.");
  expect(screen.queryByText("Saved to memory")).not.toBeInTheDocument();
  expect(view.editor).toHaveValue(body);
  expect(localStorage.getItem(key)).toBe(body);
  expect(view.close).not.toHaveBeenCalled();
});

it("says when the text cannot be kept on this device, without dropping it", () => {
  vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => { throw new Error("blocked"); });
  vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("quota"); });
  const { editor } = mount();
  expect(screen.getByRole("alert")).toHaveTextContent("Save it before you quit Wenlan.");
  type(editor);
  expect(editor).toHaveValue(body);
  expect(screen.getByRole("alert")).toBeVisible();
});

it("still closes after a discard when storage is blocked", async () => {
  const view = mount();
  type(view.editor);
  vi.spyOn(Storage.prototype, "removeItem").mockImplementation(() => { throw new Error("blocked"); });
  fireEvent.keyDown(view.editor, { key: "Escape" });
  await waitFor(() => expect(view.close).toHaveBeenCalledTimes(1));
});

it("keeps the ten character floor and a separate slot for the in-app dialog", () => {
  localStorage.setItem(quickCaptureDraftKey(false), "modal draft");
  const view = mount();
  expect(view.editor).toHaveValue("");
  type(view.editor, "😀".repeat(9));
  save(view.editor);
  expect(quickCapture).not.toHaveBeenCalled();
  expect(screen.getByRole("alert")).toHaveTextContent("Write at least 10 characters before saving.");
  expect(localStorage.getItem(quickCaptureDraftKey(false))).toBe("modal draft");
});

it("names the platform's modifier in the shortcut hint", () => {
  vi.spyOn(navigator, "userAgent", "get").mockReturnValue(
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 Edg/124.0.0.0",
  );
  mount();
  expect(screen.getByText("Esc close · Ctrl+Enter save")).toBeInTheDocument();
});
