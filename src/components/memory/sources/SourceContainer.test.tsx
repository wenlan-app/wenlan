// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { getKnowledgePath, getKnowledgePathInfo, getNoteAiChanges, listIndexedFiles, listNoteProposals, listRegisteredSources, readSourceDir, readSourceText, setSourceFollowChanges, syncRegisteredSource, type RegisteredSource } from "../../../lib/tauri";
import SourceLibrary, { type SourceLibraryState } from "./SourceLibrary";
import { useState } from "react";

vi.mock("../../../lib/tauri", () => ({
  getActiveImportBatches: vi.fn().mockResolvedValue({ batches: [] }),
  listIndexedFiles: vi.fn(),
  listRegisteredSources: vi.fn(),
  readSourceDir: vi.fn(),
  readSourceText: vi.fn(),
  syncRegisteredSource: vi.fn(),
  setSourceFollowChanges: vi.fn(),
  sourceFollowsChanges: (s: { status: unknown }) => s.status !== "Paused",
  openFile: vi.fn(),
  getKnowledgePathInfo: vi.fn(),
  setKnowledgePath: vi.fn(),
  restartDaemonToMovePages: vi.fn(),
  getKnowledgePath: vi.fn(),
  getNoteAiChanges: vi.fn(),
  setNoteAiChanges: vi.fn(),
  listNoteProposals: vi.fn(),
  getNoteProposal: vi.fn(),
  acceptNoteProposal: vi.fn(),
  dismissNoteProposal: vi.fn(),
}));

const source: RegisteredSource = {
  id: "vault-1", source_type: "obsidian", path: "/notes/Vault", status: "Active",
  last_sync: 1_728_000_000, file_count: 12, memory_count: 8,
};
const originalMatchMedia = Object.getOwnPropertyDescriptor(window, "matchMedia");

function TestLibrary() {
  const [state, setState] = useState<SourceLibraryState>({ search: "", filter: "all" });
  return <SourceLibrary onAdd={vi.fn()} onOpenDocument={vi.fn()} state={state} onStateChange={setState} />;
}

function renderLibrary() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><TestLibrary /></QueryClientProvider>);
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(listRegisteredSources).mockResolvedValue([source]);
  vi.mocked(listIndexedFiles).mockResolvedValue([]);
  vi.mocked(getKnowledgePathInfo).mockResolvedValue({ path: "/home/me/.wenlan/pages" });
  vi.mocked(readSourceDir).mockResolvedValue([]);
  vi.mocked(readSourceText).mockResolvedValue("");
  vi.mocked(syncRegisteredSource).mockResolvedValue({ files_found: 0, ingested: 0, skipped: 0, errors: 0 });
  vi.mocked(getKnowledgePath).mockResolvedValue("/home/me/.wenlan/pages");
  vi.mocked(getNoteAiChanges).mockImplementation(async (_id, path) => ({ path, value: "never", from: "default" }));
  vi.mocked(listNoteProposals).mockResolvedValue([]);
  Object.defineProperty(window, "matchMedia", { configurable: true, value: vi.fn().mockReturnValue({
    matches: true, media: "(min-width: 1100px)", addEventListener: vi.fn(), removeEventListener: vi.fn(),
  }) });
});

afterEach(() => {
  if (originalMatchMedia) Object.defineProperty(window, "matchMedia", originalMatchMedia);
  else Reflect.deleteProperty(window, "matchMedia");
});

describe("Sources containers", () => {
  it("switches a synced source to import-once from its Info panel", async () => {
    vi.mocked(setSourceFollowChanges).mockResolvedValue({ ...source, status: "Paused" });
    renderLibrary();
    fireEvent.click(await screen.findByRole("button", { name: /Vault|vault-1/i }));
    const toggle = await screen.findByRole("button", { name: "Keep in sync" });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/Checked for changes about every 30 seconds/)).toBeInTheDocument();
    fireEvent.click(toggle);
    await waitFor(() => expect(setSourceFollowChanges).toHaveBeenCalledWith("vault-1", false));
  });

  it("shows an import-once source as imported once with Refresh still available", async () => {
    vi.mocked(listRegisteredSources).mockResolvedValue([{ ...source, status: "Paused" }]);
    renderLibrary();
    fireEvent.click(await screen.findByRole("button", { name: /Vault|vault-1/i }));
    expect(await screen.findByRole("button", { name: "Keep in sync" })).toHaveAttribute("aria-pressed", "false");
    expect(screen.getByText("Imported once")).toBeInTheDocument();
    expect(screen.getByText(/Later changes wait until you Refresh/)).toBeInTheDocument();
    expect(screen.queryByText(/about every 30 seconds/)).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: /^Refresh$/ }));
    await waitFor(() => expect(syncRegisteredSource).toHaveBeenCalledWith("vault-1"));
  });

  it("opens a Vault, enters a nested folder, and returns through its breadcrumb", async () => {
    vi.mocked(readSourceDir).mockImplementation(async (path) => path === source.path
      ? [{ name: "Projects", isDirectory: true }, { name: "root.md", isDirectory: false }]
      : [{ name: "Nested", isDirectory: true }]);
    renderLibrary();

    fireEvent.click(await screen.findByRole("button", { name: /Vault|vault-1/i }));
    expect(await screen.findByText("Projects")).toBeInTheDocument();
    const info = await screen.findByText(/Wenlan changes your notes only when you accept/);
    expect(info).toBeInTheDocument();
    expect(screen.getByText("12")).toBeInTheDocument();
    expect(screen.getByText("8")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /Refresh|refresh/ }));
    await waitFor(() => expect(syncRegisteredSource).toHaveBeenCalledWith(source.id));
    fireEvent.click(screen.getByRole("button", { name: /Close/ }));
    expect(screen.queryByText(/Wenlan reads your notes here/)).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: /memoryPanel|Information|info/i }));
    expect(await screen.findByText(/Wenlan changes your notes only when you accept/)).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /Projects/ }));
    expect(await screen.findByText("Nested")).toBeInTheDocument();
    await waitFor(() => expect(readSourceDir).toHaveBeenCalledWith("/notes/Vault/Projects"));

    const breadcrumbs = screen.getByRole("navigation", { name: /sourceContainer\.breadcrumb|Breadcrumb/ });
    fireEvent.click(within(breadcrumbs).getAllByRole("button")[1]);
    expect(await screen.findByText("root.md")).toBeInTheDocument();
    await waitFor(() => expect(readSourceDir).toHaveBeenCalledWith(source.path));
  });

  it("keeps folder read errors visible and retries the same directory", async () => {
    vi.mocked(readSourceDir).mockRejectedValueOnce(new Error("unreadable")).mockResolvedValue([{ name: "Recovered", isDirectory: true }]);
    renderLibrary();
    fireEvent.click(await screen.findByRole("button", { name: /Vault|vault-1/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/Could not read this folder|sourceContainer\.folderReadError/);
    fireEvent.click(screen.getByRole("button", { name: /Try again|retryFolder/ }));
    expect(await screen.findByText("Recovered")).toBeInTheDocument();
    expect(readSourceDir).toHaveBeenCalledTimes(2);
  });

  it("reads the complete local Markdown file through the shared document layout", async () => {
    vi.mocked(readSourceDir).mockResolvedValue([{ name: "notes.md", isDirectory: false }]);
    vi.mocked(readSourceText).mockResolvedValue("# Notes\n\nFirst paragraph.\n\nEntire local file, including later paragraphs.");
    vi.mocked(listIndexedFiles).mockResolvedValue([
      { source: "memory", source_agent: "obsidian", source_id: "vault-1::/notes/Vault/notes.md::0", title: "Notes", chunk_count: 1, last_modified: 10 },
      { source: "memory", source_agent: "obsidian", source_id: "vault-1::/notes/Vault/notes.md::1", title: "Notes", chunk_count: 2, last_modified: 10 },
    ]);
    renderLibrary();

    fireEvent.click(await screen.findByRole("button", { name: /Vault|vault-1/i }));
    fireEvent.click(await screen.findByRole("button", { name: /notes\.md/ }));
    expect(await screen.findByText("Entire local file, including later paragraphs.")).toBeInTheDocument();
    expect(readSourceText).toHaveBeenCalledWith("/notes/Vault/notes.md");
    expect(document.querySelector(".reading-document-frame")).toBeInTheDocument();
    expect(await screen.findByText("In your library")).toBeInTheDocument();
    expect(screen.getByText("3 passages")).toBeInTheDocument();
  });
});
