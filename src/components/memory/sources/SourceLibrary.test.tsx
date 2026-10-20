// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getActiveImportBatches, listIndexedFiles, listRegisteredSources, type IndexedFileInfo, type RegisteredSource } from "../../../lib/tauri";
import SourceLibrary, { type SourceLibraryProps, type SourceLibraryState } from "./SourceLibrary";
import SourcesView from "../SourcesView";

vi.mock("../../../lib/tauri", () => ({ getActiveImportBatches: vi.fn().mockResolvedValue({ batches: [] }), listIndexedFiles: vi.fn(), listRegisteredSources: vi.fn() }));

const file = (patch: Partial<IndexedFileInfo> = {}): IndexedFileInfo => ({
  source: "file", source_id: "/notes/research/Design.pdf", title: "Design study", chunk_count: 1, last_modified: 1, ...patch,
});
const folder = (patch: Partial<RegisteredSource> = {}): RegisteredSource => ({
  id: "folder-notes", source_type: "directory", path: "/notes/Research", status: "Active", last_sync: 1, file_count: 5, memory_count: 20, ...patch,
});

function renderLibrary(options: Partial<SourceLibraryProps> = {}) {
  const props = { onAdd: vi.fn(), onOpenDocument: vi.fn(), ...options };
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><SourceLibrary {...props} /></QueryClientProvider>);
  return props;
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getActiveImportBatches).mockResolvedValue({ batches: [] });
  vi.mocked(listRegisteredSources).mockResolvedValue([]);
  vi.mocked(listIndexedFiles).mockResolvedValue([]);
});

describe("SourceLibrary", () => {
  it("uses supplied state and reports complete changes without overriding its owner", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([file()]);
    const onStateChange = vi.fn();
    renderLibrary({ state: { search: "design", filter: "files" }, onStateChange });
    expect(await screen.findByText("Design study")).toBeInTheDocument();
    expect(screen.getByRole("searchbox")).toHaveValue("design");
    expect(screen.getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "research" } });
    expect(onStateChange).toHaveBeenLastCalledWith({ search: "research", filter: "files" });
    expect(screen.getByRole("searchbox")).toHaveValue("design");
    fireEvent.click(screen.getByRole("button", { name: "Links" }));
    expect(onStateChange).toHaveBeenLastCalledWith({ search: "design", filter: "links" });
    expect(screen.getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
  });

  it("reports clearing controlled search and filter as one state change", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([file()]);
    const onStateChange = vi.fn();
    renderLibrary({ state: { search: "missing", filter: "files" }, onStateChange });
    fireEvent.click(await screen.findByRole("button", { name: "Clear search and filters" }));
    expect(onStateChange).toHaveBeenCalledExactlyOnceWith({ search: "", filter: "all" });
  });

  it("retains route-owned search and filter when SourcesView unmounts and returns", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([
      file(), file({ source: "webpage", source_id: "https://docs.example.com/guide", title: "Reference guide", url: "https://docs.example.com/guide" }),
    ]);
    function RouteOwner() {
      const [libraryState, setLibraryState] = useState<SourceLibraryState>({ search: "", filter: "all" });
      const [visible, setVisible] = useState(true);
      return <>
        <button onClick={() => setVisible(value => !value)}>{visible ? "Leave Sources" : "Return to Sources"}</button>
        {visible && <SourcesView onManageSources={() => {}} libraryState={libraryState} onLibraryStateChange={setLibraryState} />}
      </>;
    }
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    render(<QueryClientProvider client={client}><RouteOwner /></QueryClientProvider>);
    await screen.findByText("Reference guide");
    fireEvent.click(screen.getByRole("button", { name: "Links" }));
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "docs.example.com" } });
    expect(screen.queryByText("Design study")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Leave Sources" }));
    expect(screen.queryByRole("searchbox")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Return to Sources" }));
    expect(await screen.findByText("Reference guide")).toBeInTheDocument();
    expect(screen.getByRole("searchbox")).toHaveValue("docs.example.com");
    expect(screen.getByRole("button", { name: "Links" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByText("Design study")).not.toBeInTheDocument();
  });

  it("shows imported documents when no registered folders exist", async () => {
    const imported = file();
    vi.mocked(listIndexedFiles).mockResolvedValue([imported]);
    const props = renderLibrary();
    fireEvent.click(await screen.findByRole("button", { name: /Design study/ }));
    expect(props.onOpenDocument).toHaveBeenCalledWith(imported);
    expect(screen.getByText("PDF · research")).toBeInTheDocument();
    expect(screen.queryByText("Bring your sources together")).not.toBeInTheDocument();
  });

  it("switches the shared inventory between rows and cards and keeps New reachable", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([file()]);
    const props = renderLibrary();
    expect(await screen.findByText("Design study")).toBeInTheDocument();
    const initialRows = screen.getAllByRole("button").filter((button) => button.classList.contains("source-library-row"));
    fireEvent.click(screen.getByRole("button", { name: "Cards" }));
    expect(screen.getByRole("button", { name: "Rows" })).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByRole("button", { name: "Cards" })).toHaveAttribute("aria-pressed", "true");
    const cards = document.querySelector('.source-library-list[data-lens="cards"]');
    expect(cards?.querySelectorAll(".source-library-row")).toHaveLength(initialRows.length);
    expect(screen.queryByRole("button", { name: "More source actions" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "New" }));
    expect(props.onAdd).toHaveBeenCalledOnce();
  });

  it("includes imported source kinds and excludes normal agent memories and recaps even with URLs", async () => {
    vi.mocked(listIndexedFiles).mockResolvedValue([
      file({ source: "directory", source_id: "dir::/notes/a.md", title: "Directory document" }),
      file({ source: "obsidian", source_id: "vault::/vault/b.md", title: "Vault document" }),
      file({ source: "memory", source_agent: "folder", source_id: "import-a", title: "Folder import" }),
      file({ source: "memory", source_agent: "obsidian", source_id: "import-b", title: "Vault import" }),
      file({ source: "memory", source_agent: "codex", source_id: "agent", title: "Agent memory", url: "https://example.com" }),
      file({ source: "memory", source_agent: "folder", memory_type: "recap", source_id: "recap", title: "Daily recap" }),
    ]);
    renderLibrary();
    expect(await screen.findByText("Directory document")).toBeInTheDocument();
    expect(screen.getByText("Vault document")).toBeInTheDocument();
    expect(screen.getByText("Folder import")).toBeInTheDocument();
    expect(screen.getByText("Vault import")).toBeInTheDocument();
    expect(screen.queryByText("Agent memory")).not.toBeInTheDocument();
    expect(screen.queryByText("Daily recap")).not.toBeInTheDocument();
  });

  it("hides the managed uploads folder, searches paths and opens registered containers", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([
      folder(), folder({ id: "managed", path: "/Users/me/.wenlan/sources/" }),
      folder({ id: "managed-win", path: "C:\\Users\\me\\.wenlan\\sources\\" }),
    ]);
    const props = renderLibrary();
    const research = await screen.findByRole("button", { name: /Research.*Folder/ });
    expect(screen.queryByText(/\.wenlan/)).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "/notes/" } });
    expect(research).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "" } });
    fireEvent.click(research);
    expect(await screen.findByRole("navigation")).toBeInTheDocument();
    expect(screen.getByRole("navigation").querySelectorAll("button")[1]).toHaveTextContent("Research");
    fireEvent.click(screen.getByRole("button", { name: "New" }));
    expect(props.onAdd).toHaveBeenCalledOnce();
  });

  it("shows the app a saved link came from instead of the generic webpage label and host", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([folder()]);
    vi.mocked(listIndexedFiles).mockResolvedValue([
      file({ source: "webpage", source_id: "https://mail.google.com/mail/u/0/#inbox/FMfcgzQ", title: "Invoice from Acme", url: "https://mail.google.com/mail/u/0/#inbox/FMfcgzQ" }),
      file({ source: "webpage", source_id: "https://example.com/blog/local-first", title: "Local-first essay", url: "https://example.com/blog/local-first" }),
    ]);
    renderLibrary();
    const gmailRow = (await screen.findByText("Invoice from Acme")).closest("button")!;
    expect(gmailRow).toHaveTextContent(/^Invoice from AcmeGmail$/);
    expect(gmailRow.querySelector('[data-resource-icon="gmail"]')).not.toBeNull();
    expect(screen.queryByText(/mail\.google\.com/)).not.toBeInTheDocument();
    const plainRow = screen.getByText("Local-first essay").closest("button")!;
    expect(plainRow).toHaveTextContent("Webpage · example.com");
    expect(plainRow.querySelector('[data-resource-icon="webpage"]')).not.toBeNull();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "gmail" } });
    expect(screen.getByText("Invoice from Acme")).toBeInTheDocument();
    expect(screen.queryByText("Local-first essay")).not.toBeInTheDocument();
  });

  it("gives each app with saved sources its own filter and keeps them out of Links", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([folder()]);
    vi.mocked(listIndexedFiles).mockResolvedValue([
      file({ source: "webpage", source_id: "https://mail.google.com/mail/u/0/#inbox/FMfcgzQ", title: "Invoice from Acme", url: "https://mail.google.com/mail/u/0/#inbox/FMfcgzQ" }),
      file({ source: "webpage", source_id: "https://www.notion.so/team/Q4-roadmap-1a2b3c", title: "Q4 roadmap", url: "https://www.notion.so/team/Q4-roadmap-1a2b3c" }),
      file({ source: "webpage", source_id: "https://example.com/blog/local-first", title: "Local-first essay", url: "https://example.com/blog/local-first" }),
    ]);
    renderLibrary();
    await screen.findByText("Invoice from Acme");
    const filters = screen.getByRole("group", { name: "Filter sources" });
    expect([...filters.querySelectorAll("button")].map((button) => button.textContent)).toEqual(["All", "Files", "Links", "Gmail", "Notion", "Folders"]);
    fireEvent.click(screen.getByRole("button", { name: "Links" }));
    expect(screen.getByText("Local-first essay")).toBeInTheDocument();
    expect(screen.queryByText("Invoice from Acme")).not.toBeInTheDocument();
    expect(screen.queryByText("Q4 roadmap")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Gmail" }));
    expect(screen.getByText("Invoice from Acme")).toBeInTheDocument();
    expect(screen.queryByText("Q4 roadmap")).not.toBeInTheDocument();
    expect(screen.queryByText("Local-first essay")).not.toBeInTheDocument();
    expect(screen.queryByText("Research")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Notion" }));
    expect(screen.getByText("Q4 roadmap")).toBeInTheDocument();
    expect(screen.queryByText("Invoice from Acme")).not.toBeInTheDocument();
  });

  it("filters links using webpage or http URL and searches URL/title/path", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([folder()]);
    vi.mocked(listIndexedFiles).mockResolvedValue([
      file(),
      file({ source: "webpage", source_id: "https://docs.example.com/guide", title: "Reference guide", url: "https://docs.example.com/guide" }),
      file({ source: "directory", source_id: "website", title: "Saved article", url: "https://news.example.com/article" }),
      file({ source_id: "local", title: "Local note.txt", url: "file:///notes/local.txt" }),
    ]);
    renderLibrary();
    await screen.findByText("Reference guide");
    fireEvent.click(screen.getByRole("button", { name: "Links" }));
    expect(screen.getByText("Webpage · docs.example.com")).toBeInTheDocument();
    expect(screen.getByText("Saved article")).toBeInTheDocument();
    expect(screen.queryByText("Design study")).not.toBeInTheDocument();
    expect(screen.queryByText("Research")).not.toBeInTheDocument();
    expect(screen.queryByText("Local note.txt")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "docs.example.com/guide" } });
    expect(screen.getByText("Reference guide")).toBeInTheDocument();
    expect(screen.queryByText("Saved article")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "missing" } });
    expect(screen.getByText("No matching sources")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Clear search and filters" }));
    fireEvent.click(screen.getByRole("button", { name: "Files" }));
    expect(screen.getByText("Design study")).toBeInTheDocument();
    expect(screen.getByText("Local note.txt")).toBeInTheDocument();
    expect(screen.queryByText("Reference guide")).not.toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "/research/design.pdf" } });
    expect(screen.getByText("Design study")).toBeInTheDocument();
    fireEvent.change(screen.getByRole("searchbox"), { target: { value: "" } });
    fireEvent.click(screen.getByRole("button", { name: "Folders" }));
    expect(screen.getByText("Research")).toBeInTheDocument();
    expect(screen.queryByText("Design study")).not.toBeInTheDocument();
  });

  it("keeps a central New action and supported-file copy when truly empty", async () => {
    const props = renderLibrary();
    expect(await screen.findByText("Bring your sources together")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Sources", level: 1 })).toBeInTheDocument();
    expect(screen.getByRole("searchbox")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Folders" })).toBeInTheDocument();
    expect(screen.getByText("Supported files: PDF, Markdown and TXT.")).toBeInTheDocument();
    const emptyState = screen.getByText("Bring your sources together").parentElement!;
    fireEvent.click(within(emptyState).getByRole("button", { name: "New" }));
    expect(props.onAdd).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "Clear search and filters" })).not.toBeInTheDocument();
  });

  it("does not mistake independent pending queries for an empty library", async () => {
    vi.mocked(listRegisteredSources).mockImplementation(() => new Promise(() => {}));
    vi.mocked(listIndexedFiles).mockResolvedValue([file()]);
    renderLibrary();
    expect(await screen.findByText("Design study")).toBeInTheDocument();
    expect(screen.getByText("Loading folders…")).toBeInTheDocument();
    expect(screen.queryByText("Bring your sources together")).not.toBeInTheDocument();
  });

  it("keeps documents visible after folder failure and retries only folders", async () => {
    vi.mocked(listRegisteredSources).mockRejectedValueOnce(new Error("offline")).mockResolvedValue([folder()]);
    vi.mocked(listIndexedFiles).mockResolvedValue([file()]);
    renderLibrary();
    expect(await screen.findByText("Could not load your folders.")).toBeInTheDocument();
    expect(screen.getByText("Design study")).toBeInTheDocument();
    expect(screen.queryByText("Bring your sources together")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry folders" }));
    expect(await screen.findByText("Research")).toBeInTheDocument();
    expect(listRegisteredSources).toHaveBeenCalledTimes(2);
    expect(listIndexedFiles).toHaveBeenCalledOnce();
  });

  it("keeps folders visible after document failure and independently retries documents", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([folder()]);
    vi.mocked(listIndexedFiles).mockRejectedValueOnce(new Error("offline")).mockResolvedValue([file()]);
    renderLibrary();
    expect(await screen.findByText("Could not load your documents.")).toBeInTheDocument();
    expect(screen.getByText("Research")).toBeInTheDocument();
    expect(screen.queryByText("Bring your sources together")).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry documents" }));
    expect(await screen.findByText("Design study")).toBeInTheDocument();
    expect(listRegisteredSources).toHaveBeenCalledOnce();
    expect(listIndexedFiles).toHaveBeenCalledTimes(2);
  });

  it("shows both failed queries without a false empty or filter-empty state", async () => {
    vi.mocked(listRegisteredSources).mockRejectedValue(new Error("offline"));
    vi.mocked(listIndexedFiles).mockRejectedValue(new Error("offline"));
    renderLibrary();
    await waitFor(() => expect(screen.getAllByRole("alert")).toHaveLength(2));
    expect(screen.queryByText("Bring your sources together")).not.toBeInTheDocument();
    expect(screen.queryByText("No matching sources")).not.toBeInTheDocument();
  });
});
