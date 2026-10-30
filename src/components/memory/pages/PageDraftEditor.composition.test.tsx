// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createRef } from "react";
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { createPageDraft, publishPageDraft, type Page } from "../../../lib/tauri";
import { PageDraftEditor, type PageDraftEditorHandle } from "./PageDraftEditor";

vi.mock("../../../lib/tauri", () => ({
  createPageDraft: vi.fn(),
  discardPageDraft: vi.fn(),
  getPage: vi.fn(),
  publishPageDraft: vi.fn(),
  updatePageDraft: vi.fn(),
}));

function renderEditor(onBack = vi.fn()) {
  const ref = createRef<PageDraftEditorHandle>();
  render(
    <QueryClientProvider client={new QueryClient()}>
      <PageDraftEditor ref={ref} space={null} onBack={onBack} onOpenExisting={vi.fn()} onPublished={vi.fn()} />
    </QueryClientProvider>,
  );
  return ref;
}

beforeEach(() => {
  vi.mocked(createPageDraft).mockReset().mockImplementation(async (input) => ({
    id: input.clientDraftId, title: input.title, content: input.content, space: input.space,
    version: 1, status: "draft", creation_kind: "authored",
  }) as Page);
  vi.mocked(publishPageDraft).mockReset().mockImplementation(async (input) => ({
    id: input.id, title: "中文標題", content: "", version: 2, status: "active", creation_kind: "authored",
  }) as Page);
});
afterEach(() => vi.useRealTimers());

it.each(["Title", "Content"])("does not autosave or finalize while the %s field composes", async (name) => {
  vi.useFakeTimers();
  const ref = renderEditor();
  const field = screen.getByRole("textbox", { name });
  fireEvent.compositionStart(field);
  fireEvent.change(field, { target: { value: "ㄓㄨㄥ" } });
  await act(async () => {
    await vi.advanceTimersByTimeAsync(1000);
    expect(await ref.current?.flush()).toBe(false);
  });
  expect(createPageDraft).not.toHaveBeenCalled();

  fireEvent.change(field, { target: { value: "中文標題" } });
  fireEvent.compositionEnd(field, { data: "中文標題" });
  await act(async () => { await vi.advanceTimersByTimeAsync(701); });
  expect(createPageDraft).toHaveBeenCalledWith(expect.objectContaining(
    name === "Title" ? { title: "中文標題" } : { content: "中文標題" },
  ));
});

it("keeps Escape for the IME while the title composes", () => {
  const onBack = vi.fn();
  renderEditor(onBack);
  const title = screen.getByRole("textbox", { name: "Title" });
  fireEvent.compositionStart(title);
  const escape = fireEvent.keyDown(title, { key: "Escape" });
  expect(escape).toBe(true);
  expect(onBack).not.toHaveBeenCalled();
});

it("treats the preedit as typed text once the field loses focus without compositionend", async () => {
  const ref = renderEditor();
  const title = screen.getByRole("textbox", { name: "Title" });
  title.focus();
  fireEvent.compositionStart(title);
  fireEvent.change(title, { target: { value: "中文標題" } });
  act(() => title.blur());
  await act(async () => { expect(await ref.current?.flush()).toBe(true); });
  expect(createPageDraft).toHaveBeenCalledWith(expect.objectContaining({ title: "中文標題" }));
  expect(publishPageDraft).toHaveBeenCalledTimes(1);
});
