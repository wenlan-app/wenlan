// SPDX-License-Identifier: AGPL-3.0-only
import { beforeAll, beforeEach, describe, expect, it, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import type { ReactNode } from "react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getSourceAnnotation, listPages, updateSourceAnnotation, type SourceAnnotation } from "../../../lib/tauri";
import SourceAnnotations from "./SourceAnnotations";
import { editorViewFromTextbox, installCodeMirrorDomPolyfills, replaceDocument } from "../editor/editorTestUtils";

beforeAll(() => installCodeMirrorDomPolyfills());

const editorLabel = "Source description";
const editorByRole = () => screen.getByRole("textbox", { name: editorLabel });
const editorText = (editor: HTMLElement = editorByRole()) => editorViewFromTextbox(editor).state.doc.toString();
const changeEditor = (editor: HTMLElement, value: string) => act(() => replaceDocument(editorViewFromTextbox(editor), value));

vi.mock("../../../lib/tauri", () => ({
  getSourceAnnotation: vi.fn(), listPages: vi.fn(), updateSourceAnnotation: vi.fn(),
}));

const empty = (patch: Partial<SourceAnnotation> = {}): SourceAnnotation => ({
  source: "webpage", source_id: "https://example.com/a", notes: "", page_ids: [], version: 0, updated_at: 0, pages: [], ...patch,
});

function renderNotes(options: { sourceId?: string; space?: string; global?: boolean; renderRelatedPages?: (content: ReactNode) => ReactNode; onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void } = {}) {
  const sourceId = options.sourceId ?? "https://example.com/a";
  const space = options.global ? undefined : options.space ?? "Reading";
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  const rendered = render(<QueryClientProvider client={queryClient}><SourceAnnotations sourceId={sourceId} space={space} renderRelatedPages={options.renderRelatedPages} onRegisterFlush={options.onRegisterFlush} /></QueryClientProvider>);
  return { ...rendered, queryClient, sourceId, space };
}

beforeEach(() => {
  vi.resetAllMocks();
  sessionStorage.clear();
  vi.mocked(getSourceAnnotation).mockResolvedValue(empty());
  vi.mocked(listPages).mockResolvedValue([]);
  vi.mocked(updateSourceAnnotation).mockImplementation(async (request) => empty({
    notes: request.notes ?? "", page_ids: request.page_ids ?? [], version: request.expected_version + 1,
  }));
});

describe("SourceAnnotations", () => {
  it("drains a newer Description edit entered during an in-flight save before resolving its flush", async () => {
    let flush: (() => Promise<boolean>) | null = null;
    renderNotes({ onRegisterFlush: (next) => { flush = next; } });
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editor, "First draft");
    let finishFirst!: (value: SourceAnnotation) => void;
    vi.mocked(updateSourceAnnotation).mockImplementationOnce(() => new Promise((resolve) => { finishFirst = resolve; }));

    let completed = false;
    const pending = flush!().then((saved) => { completed = saved; });
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(1));
    changeEditor(editor, "Newer draft");
    finishFirst(empty({ notes: "First draft", version: 1 }));
    await pending;

    expect(completed).toBe(true);
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(2);
    expect(updateSourceAnnotation).toHaveBeenLastCalledWith(expect.objectContaining({ expected_version: 1, notes: "Newer draft" }));
    expect(editorText(editor)).toBe("Newer draft");
  });

  it("refuses a failed flush and keeps the editable Description draft visible", async () => {
    let flush: (() => Promise<boolean>) | null = null;
    renderNotes({ onRegisterFlush: (next) => { flush = next; } });
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editor, "Keep this draft");
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));

    let saved = true;
    await act(async () => { saved = await flush!(); });
    expect(saved).toBe(false);
    expect(editorText(editor)).toBe("Keep this draft");
    expect(await screen.findByRole("status")).toHaveTextContent("Could not save. Your text is still here.");
  });

  it("waits for an in-flight Page toggle and refuses flush when that write fails", async () => {
    let flush: (() => Promise<boolean>) | null = null;
    vi.mocked(listPages).mockResolvedValue([{ id: "page-1", title: "Reading practice", status: "active", entity_id: null } as never]);
    renderNotes({ onRegisterFlush: (next) => { flush = next; } });
    await screen.findByRole("textbox", { name: editorLabel });
    fireEvent.click(await screen.findByRole("button", { name: "+ Link a page" }));
    let rejectToggle!: (error: Error) => void;
    vi.mocked(updateSourceAnnotation).mockImplementationOnce(() => new Promise((_, reject) => { rejectToggle = reject; }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Reading practice/ }));

    let completed = false;
    let pending!: Promise<void>;
    act(() => { pending = flush!().then((saved) => { completed = saved; }); });
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(1));
    await act(async () => { rejectToggle(new Error("offline")); await pending; });
    expect(completed).toBe(false);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not update the linked page.");
  });

  it("opens directly into a Markdown Description and autosaves blockquotes without an Add reflection action", async () => {
    renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const editor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(editor, "> A useful line from the article.");
    expect(editor.querySelector(".cm-writing-blockquote")).not.toBeNull();
    expect(screen.queryByRole("button", { name: /Add a note|Add reflection/i })).not.toBeInTheDocument();
    fireEvent.blur(editor);
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledWith({
      source: "webpage", source_id: "https://example.com/a", space: "Reading", expected_version: 0,
      notes: "> A useful line from the article.",
    }));
    expect(await screen.findByText("Saved")).toBeInTheDocument();
  });

  it("shows the initial load error and retries when no cached Description exists", async () => {
    vi.mocked(getSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    renderNotes();
    expect(await screen.findByText("Could not load the source description.")).toBeInTheDocument();
    expect(screen.queryByRole("textbox", { name: editorLabel })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("textbox", { name: editorLabel })).toBeInTheDocument();
  });

  it("adopts a changed clean server Description in a fresh editor session without writing", async () => {
    const { queryClient } = renderNotes();
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    expect(editorText(editor)).toBe("");
    act(() => queryClient.setQueryData(
      ["sourceAnnotation", "webpage", "https://example.com/a", "Reading"],
      empty({ notes: "Updated on another surface", version: 1 }),
    ));
    await waitFor(() => expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("Updated on another surface"));
    expect(updateSourceAnnotation).not.toHaveBeenCalled();
  });

  it("keeps cached Description drafts editable when a background refetch fails", async () => {
    vi.mocked(getSourceAnnotation)
      .mockResolvedValueOnce(empty({ notes: "Cached description", version: 2 }))
      .mockRejectedValueOnce(new Error("offline"));
    const { queryClient } = renderNotes();
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editor, "Unsaved local description");
    await act(async () => {
      await queryClient.refetchQueries({ queryKey: ["sourceAnnotation", "webpage", "https://example.com/a", "Reading"] });
    });
    expect(screen.getByRole("textbox", { name: editorLabel })).toBeInTheDocument();
    expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("Unsaved local description");
    expect(getSourceAnnotation).toHaveBeenCalledTimes(2);
    expect(updateSourceAnnotation).not.toHaveBeenCalled();
  });

  it("renders the existing Page controls through the supplied Info wrapper", async () => {
    vi.mocked(getSourceAnnotation).mockResolvedValue(empty({
      page_ids: ["page-1"], pages: [{ id: "page-1", title: "Reading practice" }],
    }));
    renderNotes({ renderRelatedPages: (content) => <aside data-testid="info-pages">{content}</aside> });
    const wrapper = await screen.findByTestId("info-pages");
    expect(within(wrapper).getByRole("region", { name: "Pages" })).toBeInTheDocument();
    expect(await within(wrapper).findByText("Reading practice")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Description" })).not.toContainElement(wrapper);
  });

  it("does not autosave an IME composition until composition ends", async () => {
    renderNotes();
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    const view = editorViewFromTextbox(editor);
    act(() => fireEvent.compositionStart(editor));
    changeEditor(editor, "漢字");
    expect(updateSourceAnnotation).not.toHaveBeenCalled();
    act(() => fireEvent.compositionEnd(editor));
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledWith(expect.objectContaining({ notes: "漢字" })), { timeout: 2_000 });
    expect(view.state.doc.toString()).toBe("漢字");
  });

  it("clears a page by saving an empty string without a delete confirmation", async () => {
    vi.mocked(getSourceAnnotation).mockResolvedValue(empty({ notes: "An earlier thought.", version: 4 }));
    renderNotes();
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editor, "");
    fireEvent.blur(editor);
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledWith(expect.objectContaining({ expected_version: 4, notes: "" })));
    expect(await screen.findByRole("textbox", { name: editorLabel })).toBeInTheDocument();
  });

  it("keeps the user's draft visible after a save failure and allows retry", async () => {
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const editor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(editor, "Keep this draft.");
    fireEvent.blur(editor);
    const retry = await screen.findByRole("button", { name: "Retry save" });
    expect(editorText(editor)).toBe("Keep this draft.");
    fireEvent.click(retry);
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(2));
    expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("Keep this draft.");
  });

  it("recovers a failed draft after navigating away and back, then clears recovery after save", async () => {
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    const first = renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const editor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(editor, "Survive in-app navigation.");
    fireEvent.blur(editor);
    await screen.findByRole("button", { name: "Retry save" });
    first.unmount();

    const second = renderNotes();
    const recovered = await screen.findByRole("textbox", { name: editorLabel });
    expect(editorText(recovered)).toBe("Survive in-app navigation.");
    expect(screen.getByRole("button", { name: "Retry save" })).toBeInTheDocument();
    await new Promise((resolve) => window.setTimeout(resolve, 800));
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Retry save" }));
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(2));
    expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("Survive in-app navigation.");
    second.unmount();

    renderNotes();
    expect(await screen.findByRole("textbox", { name: editorLabel })).toBeInTheDocument();
    expect(editorText(await screen.findByRole("textbox", { name: editorLabel }))).toBe("");
  });

  it("keeps a newer remounted Description when an older pending write resolves", async () => {
    let resolveOldSave!: (value: SourceAnnotation) => void;
    vi.mocked(getSourceAnnotation)
      .mockResolvedValueOnce(empty())
      .mockResolvedValueOnce(empty())
      .mockResolvedValueOnce(empty({ notes: "Version one", version: 1 }));
    vi.mocked(updateSourceAnnotation)
      .mockImplementationOnce(() => new Promise((resolve) => { resolveOldSave = resolve; }))
      .mockRejectedValueOnce(new Error("offline"));

    const first = renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const firstEditor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(firstEditor, "Version one");
    fireEvent.blur(firstEditor);
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(1));
    first.unmount();

    const second = renderNotes();
    const secondEditor = await screen.findByRole("textbox", { name: editorLabel });
    expect(editorText(secondEditor)).toBe("Version one");
    changeEditor(secondEditor, "Version two");
    resolveOldSave(empty({ notes: "Version one", version: 1 }));
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(2), { timeout: 2_000 });
    expect(await screen.findByRole("button", { name: "Retry save" })).toBeInTheDocument();
    expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("Version two");
    second.unmount();

    renderNotes();
    expect(editorText(await screen.findByRole("textbox", { name: editorLabel }))).toBe("Version two");
    expect(await screen.findByRole("alert")).toHaveTextContent("This description changed elsewhere");
  });

  it("isolates recovery by source and by global versus a Space named global", async () => {
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    const first = renderNotes({ sourceId: "https://example.com/a", space: "Reading" });
    await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editorByRole(), "Only Reading.");
    fireEvent.blur(screen.getByRole("textbox", { name: editorLabel }));
    await screen.findByRole("button", { name: "Retry save" });
    first.unmount();

    renderNotes({ sourceId: "https://example.com/b", space: "Reading" });
    expect(await screen.findByRole("textbox", { name: editorLabel })).toBeInTheDocument();
    expect(editorText()).toBe("");
    cleanup();

    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    const globalPage = renderNotes({ global: true });
    await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editorByRole(), "Global only.");
    fireEvent.blur(screen.getByRole("textbox", { name: editorLabel }));
    await screen.findByRole("button", { name: "Retry save" });
    globalPage.unmount();

    renderNotes({ space: "global" });
    expect(await screen.findByRole("textbox", { name: editorLabel })).toBeInTheDocument();
    expect(editorText()).toBe("");
  });

  it("restores an empty-string Description draft and reports a conflict when server content changed", async () => {
    vi.mocked(getSourceAnnotation).mockResolvedValueOnce(empty({ notes: "Old page", version: 4 }));
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    const first = renderNotes();
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editor, "");
    fireEvent.blur(editor);
    await screen.findByRole("button", { name: "Retry save" });
    first.unmount();

    vi.mocked(getSourceAnnotation).mockResolvedValueOnce(empty({ notes: "Different saved page", version: 5 }));
    renderNotes();
    expect(editorText(await screen.findByRole("textbox", { name: editorLabel }))).toBe("");
    expect(await screen.findByRole("alert")).toHaveTextContent("This description changed elsewhere");
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(1);
  });

  it("clears the recovery draft after an explicitly retried empty-string save succeeds", async () => {
    vi.mocked(getSourceAnnotation)
      .mockResolvedValueOnce(empty({ notes: "Old page", version: 4 }))
      .mockResolvedValueOnce(empty({ notes: "Old page", version: 4 }))
      .mockResolvedValueOnce(empty({ notes: "", version: 5 }));
    vi.mocked(updateSourceAnnotation)
      .mockRejectedValueOnce(new Error("offline"))
      .mockResolvedValueOnce(empty({ notes: "", version: 5 }));
    const first = renderNotes();
    const editor = await screen.findByRole("textbox", { name: editorLabel });
    changeEditor(editor, "");
    fireEvent.blur(editor);
    await screen.findByRole("button", { name: "Retry save" });
    first.unmount();

    const second = renderNotes();
    expect(editorText(await screen.findByRole("textbox", { name: editorLabel }))).toBe("");
    fireEvent.click(screen.getByRole("button", { name: "Retry save" }));
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(2));
    second.unmount();

    renderNotes();
    expect(await screen.findByRole("textbox", { name: editorLabel })).toBeInTheDocument();
    expect(editorText(await screen.findByRole("textbox", { name: editorLabel }))).toBe("");
  });

  it("does not truncate an oversized live draft or attempt a save", async () => {
    renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const text = "界".repeat(22_000);
    const editor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(editor, text);
    fireEvent.blur(editor);
    expect(editorText(editor)).toBe(text);
    expect(await screen.findByRole("alert")).toHaveTextContent("too large to keep as a recovery draft");
    expect(updateSourceAnnotation).not.toHaveBeenCalled();
  });

  it("keeps the live draft usable when session storage is unavailable", async () => {
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new DOMException("blocked", "QuotaExceededError"); });
    renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const editor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(editor, "Still editable while recovery storage is blocked.");
    expect(editorText(editor)).toBe("Still editable while recovery storage is blocked.");
    expect(await screen.findByRole("alert")).toHaveTextContent("could not be kept for recovery");
  });

  it("does not retry a failed page write on its own after the visible retry action appears", async () => {
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const editor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(editor, "A draft that must stay put.");
    fireEvent.blur(editor);
    await screen.findByRole("button", { name: "Retry save" });
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(1);

    vi.useFakeTimers();
    try {
      await act(async () => { vi.advanceTimersByTime(5_000); });
      expect(updateSourceAnnotation).toHaveBeenCalledTimes(1);
      fireEvent.click(screen.getByRole("button", { name: "Retry save" }));
      await act(async () => { await Promise.resolve(); await Promise.resolve(); });
      expect(updateSourceAnnotation).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("loads scoped Pages and links them without changing the page field", async () => {
    vi.mocked(listPages).mockResolvedValue([{
      id: "page-1", title: "Reading practice", status: "active", entity_id: null,
    } as never]);
    renderNotes();
    fireEvent.click(await screen.findByRole("button", { name: "+ Link a page" }));
    const checkbox = await screen.findByRole("checkbox", { name: /Reading practice/ });
    fireEvent.click(checkbox);
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledWith({
      source: "webpage", source_id: "https://example.com/a", space: "Reading", expected_version: 0,
      page_ids: ["page-1"],
    }));
    expect(listPages).toHaveBeenCalledWith("active", "Reading", 50, 0);
  });

  it("adopts a clean server Description while rebasing a stale Page attach", async () => {
    vi.mocked(listPages).mockResolvedValue([{
      id: "page-1", title: "Reading practice", status: "active", entity_id: null,
    } as never]);
    vi.mocked(updateSourceAnnotation)
      .mockRejectedValueOnce(new Error("409 Conflict"))
      .mockResolvedValueOnce(empty({ notes: "Server-side Description", page_ids: ["page-1"], version: 2 }));
    vi.mocked(getSourceAnnotation)
      .mockResolvedValueOnce(empty())
      .mockResolvedValueOnce(empty({ notes: "Server-side Description", version: 1 }));
    renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    fireEvent.click(screen.getByRole("button", { name: "+ Link a page" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Reading practice/ }));
    await waitFor(() => expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("Server-side Description"));
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(2);
    expect(updateSourceAnnotation).toHaveBeenLastCalledWith(expect.objectContaining({
      expected_version: 1, page_ids: ["page-1"],
    }));
    expect(vi.mocked(updateSourceAnnotation).mock.calls[1]?.[0]).not.toHaveProperty("notes");
  });

  it("preserves an attach intent if another writer has already attached the same Page", async () => {
    vi.mocked(listPages).mockResolvedValue([{
      id: "page-1", title: "Reading practice", status: "active", entity_id: null,
    } as never]);
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("409 Conflict"));
    vi.mocked(getSourceAnnotation)
      .mockResolvedValueOnce(empty())
      .mockResolvedValueOnce(empty({ page_ids: ["page-1"], pages: [{ id: "page-1", title: "Reading practice" }], version: 1 }));
    renderNotes();
    fireEvent.click(await screen.findByRole("button", { name: "+ Link a page" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Reading practice/ }));
    await waitFor(() => expect(screen.getByRole("checkbox", { name: /Reading practice/ })).toBeChecked());
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(1);
    expect(updateSourceAnnotation).toHaveBeenCalledWith(expect.objectContaining({ page_ids: ["page-1"] }));
  });

  it("shows a Page-link failure and waits for explicit retry", async () => {
    vi.mocked(listPages).mockResolvedValue([{
      id: "page-1", title: "Reading practice", status: "active", entity_id: null,
    } as never]);
    vi.mocked(updateSourceAnnotation).mockRejectedValueOnce(new Error("offline"));
    renderNotes();
    fireEvent.click(await screen.findByRole("button", { name: "+ Link a page" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Reading practice/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not update the linked page.");
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(1);
    await new Promise((resolve) => window.setTimeout(resolve, 800));
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(1);
    fireEvent.click(screen.getByRole("button", { name: "Retry save" }));
    await waitFor(() => expect(updateSourceAnnotation).toHaveBeenCalledTimes(2));
  });

  it("preserves a dirty Description conflict across a Page association rebase", async () => {
    vi.mocked(listPages).mockResolvedValue([{
      id: "page-1", title: "Reading practice", status: "active", entity_id: null,
    } as never]);
    vi.mocked(updateSourceAnnotation)
      .mockRejectedValueOnce(new Error("409 Conflict"))
      .mockResolvedValueOnce(empty({ notes: "Another saved page", page_ids: ["page-1"], pages: [{ id: "page-1", title: "Reading practice" }], version: 2 }))
      .mockResolvedValueOnce(empty({ notes: "Another saved page", version: 3 }));
    vi.mocked(getSourceAnnotation)
      .mockResolvedValueOnce(empty())
      .mockResolvedValueOnce(empty({ notes: "Another saved page", version: 1 }));
    renderNotes();
    await screen.findByRole("textbox", { name: editorLabel });
    const editor = screen.getByRole("textbox", { name: editorLabel });
    changeEditor(editor, "My unsaved page");
    fireEvent.click(screen.getByRole("button", { name: "+ Link a page" }));
    fireEvent.click(await screen.findByRole("checkbox", { name: /Reading practice/ }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This description changed elsewhere");
    expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("My unsaved page");
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(2);
    expect(updateSourceAnnotation).toHaveBeenLastCalledWith(expect.objectContaining({
      expected_version: 1, page_ids: ["page-1"],
    }));
    await new Promise((resolve) => window.setTimeout(resolve, 800));
    expect(updateSourceAnnotation).toHaveBeenCalledTimes(2);
    fireEvent.click(screen.getByRole("button", { name: "Unlink Reading practice" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("This description changed elsewhere"));
    expect(editorText(screen.getByRole("textbox", { name: editorLabel }))).toBe("My unsaved page");
  });
});
