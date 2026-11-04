import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getPage, getPageSources, getZoteroDocument, type Page } from "../../../lib/tauri";
import PageExportDialog from "./PageExportDialog";

vi.mock("../../../lib/tauri");

const page: Page = {
  id: "page-1", title: "Research Notes", content: "Saved text [1]", summary: "Summary", domain: "Research", space: "Research",
  entity_id: null, version: 4, status: "active", created_at: "2026-11-02T10:00:00Z", last_compiled: "2026-11-02T10:00:00Z",
  last_modified: "2026-11-03T12:00:00Z", source_memory_ids: [],
  citations: [{ occurrence: 1, marker: 1, source_kind: "memory", locator: "zotero::ITEM1", score: 1, status: "verified", scope: "sentence" }],
};
const source = {
  source: { page_id: page.id, memory_source_id: "zotero::ITEM1", linked_at: 1 },
  memory: { source_id: "zotero::ITEM1", title: "Paper title", content: "source", summary: null, memory_type: "research", domain: "Research", space: "Research", source_agent: "zotero", confidence: 1, confirmed: true, pinned: false, supersedes: null, last_modified: 1, chunk_count: 1 },
};

function wrap() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><PageExportDialog pageId={page.id} onClose={vi.fn()} /></QueryClientProvider>);
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getPage).mockResolvedValue(page);
  vi.mocked(getPageSources).mockResolvedValue([source]);
  vi.mocked(getZoteroDocument).mockResolvedValue({ source_id: "zotero::ITEM1", title: "Paper title", attachments: [], bibtex: "@article{item1, title={Paper title}}", warnings: [] });
});

describe("PageExportDialog", () => {
  it("saves the freshly fetched saved version through the chosen path", async () => {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { writeTextFile } = await import("@tauri-apps/plugin-fs");
    vi.mocked(save).mockResolvedValue("/tmp/Research-Notes.md");
    wrap();
    expect(await screen.findByText(/Saved version 4 ·/)).toBeInTheDocument();
    expect(getPage).toHaveBeenCalledWith(page.id);
    expect(getPageSources).toHaveBeenCalledWith(page.id);
    await waitFor(() => expect(getZoteroDocument).toHaveBeenCalledWith("zotero::ITEM1", "Research"));
    await waitFor(() => expect(screen.getByRole("button", { name: "Save snapshot" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save snapshot" }));
    await waitFor(() => expect(writeTextFile).toHaveBeenCalledOnce());
    expect(save).toHaveBeenCalledWith(expect.objectContaining({ defaultPath: "Research-Notes.md" }));
    expect(writeTextFile).toHaveBeenCalledWith("/tmp/Research-Notes.md", expect.stringContaining("Saved text"));
    expect(writeTextFile).not.toHaveBeenCalledWith(expect.anything(), expect.stringContaining("unsaved"));
    expect(screen.getByText("Snapshot saved.")).toBeInTheDocument();
  });

  it("keeps Markdown and HTML available while blocking incomplete BibTeX", async () => {
    vi.mocked(getZoteroDocument).mockRejectedValue(new Error("source offline"));
    wrap();
    expect(await screen.findByText("Some Zotero bibliography entries are unavailable. Markdown and HTML can still export; BibTeX needs every entry. Retry to load them.")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "BibTeX" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Try again" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Save snapshot" })).toBeEnabled();
  });

  it.each([
    ["Markdown", "Saved text", "Paper title"],
    ["HTML", "Saved text", "Paper title"],
  ])("exports %s from saved page/source metadata if a Zotero bibliography request fails", async (format, body, title) => {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { writeTextFile } = await import("@tauri-apps/plugin-fs");
    vi.mocked(save).mockResolvedValue(`/tmp/Research-Notes.${format === "HTML" ? "html" : "md"}`);
    vi.mocked(getZoteroDocument).mockRejectedValue(new Error("source offline"));
    wrap();

    expect(await screen.findByRole("alert")).toHaveTextContent("BibTeX needs every entry");
    fireEvent.click(screen.getByRole("radio", { name: format }));
    expect(screen.getByRole("button", { name: "Save snapshot" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "Save snapshot" }));

    await waitFor(() => expect(writeTextFile).toHaveBeenCalledOnce());
    expect(writeTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining(body));
    expect(writeTextFile).toHaveBeenCalledWith(expect.any(String), expect.stringContaining(title));
    expect(screen.getByRole("alert")).toHaveTextContent("BibTeX needs every entry");
    expect(screen.getByText("Snapshot saved.")).toBeInTheDocument();
  });

  it("still refuses an export if the linked source list changes while the save dialog is open", async () => {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { writeTextFile } = await import("@tauri-apps/plugin-fs");
    vi.mocked(save).mockImplementation(async () => {
      vi.mocked(getPageSources).mockResolvedValue([]);
      return "/tmp/Research-Notes.md";
    });
    vi.mocked(getZoteroDocument).mockRejectedValue(new Error("source offline"));
    wrap();

    expect(await screen.findByRole("alert")).toHaveTextContent("BibTeX needs every entry");
    fireEvent.click(screen.getByRole("button", { name: "Save snapshot" }));
    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent("The page or its sources changed"));
    expect(writeTextFile).not.toHaveBeenCalled();
  });

  it("refuses to save when the page changes while the native save dialog is open", async () => {
    const { save } = await import("@tauri-apps/plugin-dialog");
    const { writeTextFile } = await import("@tauri-apps/plugin-fs");
    vi.mocked(save).mockImplementation(async () => {
      vi.mocked(getPage).mockResolvedValue({ ...page, version: 5, content: "New body" });
      return "/tmp/Research-Notes.md";
    });
    wrap();
    await waitFor(() => expect(screen.getByRole("button", { name: "Save snapshot" })).toBeEnabled());
    fireEvent.click(screen.getByRole("button", { name: "Save snapshot" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The page or its sources changed");
    expect(writeTextFile).not.toHaveBeenCalled();
  });

  it("does not expose BibTeX when the page has no bibliography", async () => {
    vi.mocked(getPageSources).mockResolvedValue([]);
    wrap();
    expect(await screen.findByText("BibTeX is unavailable because this page has no Zotero bibliography entries.")).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "BibTeX" })).toBeDisabled();
    expect(getZoteroDocument).not.toHaveBeenCalled();
  });
});
