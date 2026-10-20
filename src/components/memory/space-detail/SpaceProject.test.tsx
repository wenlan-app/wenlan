// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getChunks, getSpace, listEntities, listIndexedFiles, listMemoriesRich, listPages, type ChunkDetail, type IndexedFileInfo, type Space } from "../../../lib/tauri";
import SpaceDetail from "../SpaceDetail";
import { SPACE_DETAIL_TEST_COPY } from "./testTranslation";

vi.mock("../../../lib/tauri", () => ({ getSpace: vi.fn(), listPages: vi.fn(), listIndexedFiles: vi.fn(), getChunks: vi.fn(), openSearchResult: vi.fn(), listEntities: vi.fn(), listMemoriesRich: vi.fn(), updateSpace: vi.fn(), deleteSpace: vi.fn(), confirmSpace: vi.fn() }));
const a: Space = { id: "stable-a", name: "讀書會", description: "Reading together", suggested: false, starred: false, sort_order: 0, memory_count: 12, entity_count: 6, created_at: 1, updated_at: 1 };
const b: Space = { ...a, id: "stable-b", name: "另一個專案" };
const source = (id: string, patch: Partial<IndexedFileInfo> = {}): IndexedFileInfo => ({ source: "file", source_id: id, title: id, chunk_count: 1, last_modified: 1, ...patch });
const chunk = (content: string): ChunkDetail => ({ id: "c", content, chunk_index: 0, chunk_type: null, language: null });
function deferred<T>() { let resolve!: (value: T) => void; const promise = new Promise<T>(r => { resolve = r; }); return { promise, resolve }; }
function workspace() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const onSelectPage = vi.fn(); const onCreatePage = vi.fn(); const onReviewAll = vi.fn(); const onOpenSource = vi.fn();
  const element = (spaceName: string) => <QueryClientProvider client={client}><SpaceDetail copy={SPACE_DETAIL_TEST_COPY} spaceName={spaceName} onBack={() => {}} onSelectPage={onSelectPage} onCreatePage={onCreatePage} onReviewAll={onReviewAll} onOpenSource={onOpenSource} /></QueryClientProvider>;
  const result = render(element(a.name));
  return { ...result, onSelectPage, onCreatePage, onReviewAll, onOpenSource, switchSpace: () => result.rerender(element(b.name)) };
}
beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getSpace).mockImplementation(async name => name === a.name ? a : b);
  vi.mocked(listPages).mockResolvedValue([]);
  vi.mocked(listIndexedFiles).mockResolvedValue([]);
  vi.mocked(getChunks).mockResolvedValue([]);
});
describe("Space pages and sources", () => {
  it("defaults to pages, creates in the current Space and never fetches standalone memories or topics", async () => {
    const result = workspace();
    expect(await screen.findByRole("tab", { name: "Pages" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getAllByRole("tab")).toHaveLength(2);
    expect(listIndexedFiles).not.toHaveBeenCalled();
    expect(listEntities).not.toHaveBeenCalled(); expect(listMemoriesRich).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "New page" }));
    expect(result.onCreatePage).toHaveBeenCalledExactlyOnceWith(a.name);
  });
  it("uses canonical Space scope and only document source kinds", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([source("Readme.md"), source("capture", { source: "memory" }), source("conversation", { source: "file", memory_type: "recap" })]);
    workspace();
    fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    expect(await screen.findByRole("button", { name: /Readme.md.*Markdown/ })).toBeInTheDocument();
    expect(listIndexedFiles).toHaveBeenCalledExactlyOnceWith(a.name);
    expect(screen.queryByText("capture")).not.toBeInTheDocument(); expect(screen.queryByText("conversation")).not.toBeInTheDocument();
  });
  it("keeps the selected lens across Pages and Sources for the same Space", async () => {
    vi.mocked(listPages).mockResolvedValue([{
      id: "note-1", title: "Project page", summary: "A useful summary", content: "Body", entity_id: null,
      domain: a.name, source_memory_ids: [], version: 1, status: "active", created_at: "2026-07-01T00:00:00Z",
      last_compiled: "2026-07-01T00:00:00Z", last_modified: "2026-07-01T00:00:00Z",
    }]);
    vi.mocked(listIndexedFiles).mockResolvedValue([source("Readme.md")]);
    workspace();
    await screen.findByRole("button", { name: /Project page/ });
    fireEvent.click(screen.getByRole("button", { name: "Cards" }));
    expect(document.querySelector(".space-dossier-page-list")).toHaveAttribute("data-lens", "cards");
    fireEvent.click(screen.getByRole("tab", { name: "Sources" }));
    await screen.findByText("Readme.md");
    expect(screen.getByRole("button", { name: "Cards" })).toHaveAttribute("aria-pressed", "true");
    expect(document.querySelector(".space-project-sources")).toHaveAttribute("data-lens", "cards");
    fireEvent.click(screen.getByRole("tab", { name: "Pages" }));
    expect(await screen.findByRole("button", { name: /Project page/ })).toBeInTheDocument();
    expect(document.querySelector(".space-dossier-page-list")).toHaveAttribute("data-lens", "cards");
  });
  it("keeps loading, failed and empty inventory distinct and retries the same Space", async () => {
    const pending = deferred<IndexedFileInfo[]>();
    vi.mocked(listIndexedFiles).mockReturnValueOnce(pending.promise).mockResolvedValueOnce([]);
    workspace(); fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    expect(screen.getByRole("status")).toBeInTheDocument();
    expect(screen.queryByText("No sources in this space yet.")).not.toBeInTheDocument();
    await act(async () => pending.resolve([]));
    expect(await screen.findByText("No sources in this space yet.")).toBeInTheDocument();
  });
  it("shows inventory failure and retries without inventing an empty state", async () => {
    vi.mocked(listIndexedFiles).mockRejectedValueOnce(new Error("unavailable")).mockResolvedValueOnce([source("Recovered.pdf")]);
    workspace(); fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not load sources for this space.");
    expect(screen.queryByText("No sources in this space yet.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await screen.findByText("Recovered.pdf");
    expect(listIndexedFiles).toHaveBeenNthCalledWith(2, a.name);
  });
  it("opens a scoped preview and closes it without leaving the source tab", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([source("Book.pdf")]);
    vi.mocked(getChunks).mockResolvedValue([chunk("Only this project text.")]);
    workspace(); fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    fireEvent.click(await screen.findByText("Book.pdf"));
    expect(await screen.findByText("Only this project text.")).toBeInTheDocument();
    expect(getChunks).toHaveBeenCalledExactlyOnceWith("file", "Book.pdf", a.name);
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Sources" })).toHaveAttribute("aria-selected", "true");
  });
  it("opens the source page from the preview while retaining the current Space scope", async () => {
    const file = source("shared.md");
    vi.mocked(listIndexedFiles).mockResolvedValue([file]);
    vi.mocked(getChunks).mockResolvedValue([chunk("Only this Space text.")]);
    const result = workspace();
    fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    fireEvent.click(await screen.findByRole("button", { name: /shared.md.*Markdown/ }));
    await screen.findByText("Only this Space text.");
    expect(screen.queryByRole("button", { name: "Expand reading" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Go to source" }));
    expect(result.onOpenSource).toHaveBeenCalledExactlyOnceWith({ ...file, space: a.name });
  });
  it("returns focus to the activating source row after explicit preview close", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([source("Book.pdf")]);
    vi.mocked(getChunks).mockResolvedValue([chunk("Only this project text.")]);
    workspace();
    fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    const row = await screen.findByRole("button", { name: /Book.pdf.*PDF/ });
    fireEvent.click(row);
    await screen.findByText("Only this project text.");
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await waitFor(() => expect(row).toHaveFocus());
  });
  it("resets tabs and removes old preview when Space changes while chunks are pending", async () => {
    const pending = deferred<ChunkDetail[]>();
    vi.mocked(listIndexedFiles).mockImplementation(async name => [source(name === a.name ? "A.pdf" : "B.pdf")]);
    vi.mocked(getChunks).mockReturnValueOnce(pending.promise).mockResolvedValue([chunk("New project body.")]);
    const result = workspace(); fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    fireEvent.click(await screen.findByText("A.pdf"));
    await screen.findByRole("dialog"); result.switchSpace();
    await screen.findByRole("heading", { name: b.name });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(screen.getByRole("tab", { name: "Pages" })).toHaveAttribute("aria-selected", "true");
    await act(async () => pending.resolve([chunk("Stale project body.")]));
    expect(screen.queryByText("Stale project body.")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("tab", { name: "Sources" }));
    expect(await screen.findByText("B.pdf")).toBeInTheDocument();
    expect(screen.queryByText("A.pdf")).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("B.pdf"));
    await screen.findByText("New project body.");
    expect(getChunks).toHaveBeenLastCalledWith("file", "B.pdf", b.name);
  });
  it("ignores a late inventory from the previous Space", async () => {
    const pending = deferred<IndexedFileInfo[]>();
    vi.mocked(listIndexedFiles).mockReturnValueOnce(pending.promise).mockResolvedValue([source("New.pdf")]);
    const result = workspace(); fireEvent.click(await screen.findByRole("tab", { name: "Sources" }));
    result.switchSpace(); await screen.findByRole("heading", { name: b.name });
    fireEvent.click(screen.getByRole("tab", { name: "Sources" })); await screen.findByText("New.pdf");
    await act(async () => pending.resolve([source("Old.pdf")]));
    expect(screen.queryByText("Old.pdf")).not.toBeInTheDocument();
  });
  it("uses keyboard tabs and keeps review only in the overflow menu", async () => {
    const result = workspace(); const notes = await screen.findByRole("tab", { name: "Pages" });
    notes.focus(); fireEvent.keyDown(notes, { key: "ArrowRight" });
    expect(screen.getByRole("tab", { name: "Sources" })).toHaveFocus();
    fireEvent.keyDown(screen.getByRole("tab", { name: "Sources" }), { key: "Home" });
    expect(notes).toHaveFocus();
    const more = screen.getByRole("button", { name: `Actions for ${a.name}` });
    more.focus(); fireEvent.keyDown(more, { key: "ArrowDown" });
    expect(await screen.findByRole("menuitem", { name: "Review page changes" })).toHaveFocus();
    fireEvent.click(screen.getByRole("menuitem", { name: "Review page changes" }));
    expect(result.onReviewAll).toHaveBeenCalledExactlyOnceWith();
    expect(screen.queryByRole("menu")).not.toBeInTheDocument();
  });
});
