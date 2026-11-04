import { describe, it, expect, vi, beforeEach } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import SourcesSection from "../SourcesSection";
import * as tauri from "../../../../lib/tauri";

vi.mock("../../../../lib/tauri");

function wrapper({ children }: { children: React.ReactNode }) {
  const qc = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return <QueryClientProvider client={qc}>{children}</QueryClientProvider>;
}

describe("SourcesSection", () => {
  beforeEach(() => {
    vi.mocked(tauri.getKnowledgePath).mockResolvedValue(
      "/Users/test/Wenlan/knowledge",
    );
  });

  it("renders empty state when no obsidian sources", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue([]);

    render(<SourcesSection />, { wrapper });

    await waitFor(() => {
      expect(screen.getByText("No sources yet")).toBeInTheDocument();
    });
    expect(
      screen.getByRole("button", { name: /add your first source/i }),
    ).toBeInTheDocument();
  });

  it("renders source rows when obsidian sources exist", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue([
      {
        id: "obsidian-second-brain",
        source_type: "obsidian",
        path: "/Users/test/second-brain",
        status: "Active",
        last_sync: 1712700000,
        file_count: 2146,
        memory_count: 3118,
      },
    ]);

    render(<SourcesSection />, { wrapper });

    await waitFor(() => {
      expect(screen.getByText("second-brain")).toBeInTheDocument();
    });
    expect(screen.getByText(/2,146 files/)).toBeInTheDocument();
    expect(screen.getByText(/3,118 memories/)).toBeInTheDocument();
  });

  it("shows directory sources alongside obsidian vaults", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue([
      {
        id: "obsidian-vault",
        source_type: "obsidian",
        path: "/Users/test/vault",
        status: "Active",
        last_sync: null,
        file_count: 10,
        memory_count: 20,
      },
      {
        id: "directory-papers",
        source_type: "directory",
        path: "/Users/test/papers",
        status: "Active",
        last_sync: null,
        file_count: 5,
        memory_count: 5,
      },
    ]);

    render(<SourcesSection />, { wrapper });

    await waitFor(() => {
      expect(screen.getByText("vault")).toBeInTheDocument();
    });
    expect(screen.getByText("papers")).toBeInTheDocument();
    expect(screen.getByText(/Obsidian vault ·/)).toBeInTheDocument();
    expect(screen.getByText(/Folder ·/)).toBeInTheDocument();
  });

  it("keeps page storage location out of the Sources list", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue([]);

    render(<SourcesSection />, { wrapper });

    await waitFor(() => {
      expect(screen.getByText("No sources yet")).toBeInTheDocument();
    });
    expect(screen.queryByText("Knowledge Directory")).not.toBeInTheDocument();
    expect(screen.queryByText(/Wenlan\/knowledge/)).not.toBeInTheDocument();
    expect(tauri.getKnowledgePath).not.toHaveBeenCalled();
  });

  it("offers Zotero from settings and keeps its virtual path away from Finder actions", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue([{
      id: "zotero-source", source_type: "zotero", path: "zotero-local/instance/users/0/collections/ABCD", status: "Active", last_sync: null, file_count: 2, memory_count: 2,
    }]);
    render(<SourcesSection />, { wrapper });
    expect(await screen.findByText("ABCD")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect Zotero" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Sync" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "More" }));
    expect(screen.queryByRole("menuitem", { name: "Reveal in Finder" })).not.toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Remove source" })).toBeInTheDocument();
  });
});
