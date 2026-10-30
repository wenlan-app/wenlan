// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { createRef } from "react";
import { afterEach, beforeAll, beforeEach, expect, it, vi } from "vitest";
import { createPageDraft, publishPageDraft, type Page } from "../../../lib/tauri";
import {
  editorViewFromTextbox,
  installCodeMirrorDomPolyfills,
  pressKey,
  replaceDocument,
} from "../editor/editorTestUtils";
import { PageDraftEditor, type PageDraftEditorHandle } from "./PageDraftEditor";

vi.mock("../../../lib/tauri", () => ({
  createPageDraft: vi.fn(),
  discardPageDraft: vi.fn(),
  getPage: vi.fn(),
  publishPageDraft: vi.fn(),
  updatePageDraft: vi.fn(),
}));
vi.mock("../editor/useEditorLinkNotes", () => ({ useEditorLinkNotes: () => [] }));

beforeAll(() => installCodeMirrorDomPolyfills());

beforeEach(() => {
  vi.mocked(createPageDraft).mockReset().mockImplementation(async (input) => ({
    id: input.clientDraftId, title: input.title, content: input.content, space: input.space,
    version: 1, status: "draft", creation_kind: "authored",
  }) as Page);
  vi.mocked(publishPageDraft).mockReset().mockImplementation(async (input) => ({
    id: input.id, title: "", content: "", version: 2, status: "active", creation_kind: "authored",
  }) as Page);
});
afterEach(() => vi.useRealTimers());

async function renderEditor() {
  const ref = createRef<PageDraftEditorHandle>();
  const onBack = vi.fn();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <PageDraftEditor ref={ref} space={null} onBack={onBack} onOpenExisting={vi.fn()} onPublished={vi.fn()} />
    </QueryClientProvider>,
  );
  // The first CodeMirror mount in a cold worker can take over a second when
  // the whole suite shares the CPU.
  const body = await screen.findByRole("textbox", { name: "Content" }, { timeout: 5_000 });
  return { ref, onBack, body, view: editorViewFromTextbox(body) };
}

it("writes the body in CodeMirror with the placeholder and autosaves it", async () => {
  const { body, view } = await renderEditor();
  expect(body).toHaveClass("cm-content");
  expect(body.closest(".page-draft-content")).not.toBeNull();
  expect(document.querySelector(".page-draft-content .cm-placeholder")).toHaveTextContent("Start writing…");

  act(() => replaceDocument(view, "First **paragraph**."));
  await waitFor(() => expect(createPageDraft).toHaveBeenCalledWith(
    expect.objectContaining({ content: "First **paragraph**." }),
  ));
});

it("saves a body change made in the same task as the flush, before React renders it", async () => {
  const { ref, view } = await renderEditor();
  await act(async () => {
    replaceDocument(view, "The last keystroke before the tab switch.");
    expect(await ref.current!.flush()).toBe(true);
  });
  expect(createPageDraft).toHaveBeenCalledWith(
    expect.objectContaining({ content: "The last keystroke before the tab switch." }),
  );
  expect(publishPageDraft).toHaveBeenCalledTimes(1);
});

it("holds autosave and finalize while the body composes, and releases on blur", async () => {
  const { ref, body, view } = await renderEditor();
  vi.useFakeTimers();
  fireEvent.compositionStart(body);
  act(() => replaceDocument(view, "ㄓㄨㄥ"));
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1000);
    expect(await ref.current!.flush()).toBe(false);
  });
  expect(createPageDraft).not.toHaveBeenCalled();

  // No compositionend: focus loss commits the preedit as typed text.
  act(() => replaceDocument(view, "中文內容"));
  fireEvent.blur(body);
  await act(async () => { expect(await ref.current!.flush()).toBe(true); });
  expect(createPageDraft).toHaveBeenCalledWith(expect.objectContaining({ content: "中文內容" }));
});

it("opens find with Ctrl/Cmd+F, and Escape closes it before it leaves the draft", async () => {
  const { body, onBack } = await renderEditor();
  pressKey(body, "f", { ctrlKey: true, metaKey: navigator.platform.includes("Mac") });
  const find = await screen.findByRole("textbox", { name: "Find" });
  expect(screen.getByRole("textbox", { name: "Replace" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Replace all" })).toBeInTheDocument();

  // Escape in the find field closes the panel only.
  act(() => { pressKey(find, "Escape"); });
  expect(screen.queryByRole("textbox", { name: "Find" })).not.toBeInTheDocument();
  expect(onBack).not.toHaveBeenCalled();

  // Escape in the body with the panel open also closes the panel first.
  pressKey(body, "f", { ctrlKey: true, metaKey: navigator.platform.includes("Mac") });
  await screen.findByRole("textbox", { name: "Find" });
  act(() => { pressKey(body, "Escape"); });
  expect(screen.queryByRole("textbox", { name: "Find" })).not.toBeInTheDocument();
  expect(onBack).not.toHaveBeenCalled();

  // The next Escape leaves the (empty) draft.
  await act(async () => { pressKey(body, "Escape"); });
  await waitFor(() => expect(onBack).toHaveBeenCalledTimes(1));
});
