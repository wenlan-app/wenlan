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
    expect(await screen.findByText("Zotero Collection")).toBeInTheDocument();
    expect(screen.queryByText("ABCD")).not.toBeInTheDocument();
    // Zotero is one choice of the single add-source menu, not a separate link.
    expect(screen.queryByRole("button", { name: "Connect Zotero" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Add source…" }));
    expect(await screen.findByRole("button", { name: /Zotero/ })).toBeInTheDocument();
    fireEvent.keyDown(document, { key: "Escape" });
    expect(screen.getByRole("button", { name: "Sync" })).toBeEnabled();
    fireEvent.click(screen.getByRole("button", { name: "More" }));
    expect(screen.queryByRole("menuitem", { name: "Reveal in Finder" })).not.toBeInTheDocument();
    expect(screen.getByRole("menuitem", { name: "Remove source" })).toBeInTheDocument();
  });

  it("shows a trimmed Zotero collection name without changing folder names", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue([
      { id: "zotero", source_type: "zotero", path: "zotero-local/instance/users/0/collections/ABCD", display_name: "  RAG20-research-acceptance  ", status: "Active", last_sync: null, file_count: 21, memory_count: 21 },
      { id: "folder", source_type: "directory", path: "/tmp/papers", display_name: "Ignored name", status: "Active", last_sync: null, file_count: 1, memory_count: 1 },
      { id: "legacy", source_type: "zotero", path: "zotero-local/instance/users/0/collections/LEGACY", display_name: "  ", status: "Active", last_sync: null, file_count: 0, memory_count: 0 },
    ]);
    render(<SourcesSection />, { wrapper });
    expect(await screen.findByText("RAG20-research-acceptance")).toBeInTheDocument();
    expect(screen.getByText("papers")).toBeInTheDocument();
    expect(screen.getByText("Zotero Collection")).toBeInTheDocument();
    expect(screen.queryByText("Ignored name")).not.toBeInTheDocument();
    expect(screen.queryByText("LEGACY")).not.toBeInTheDocument();
  });

  it("offers interrupted Zotero disconnection without pretending to know file counts", async () => {
    vi.mocked(tauri.listRegisteredSources).mockResolvedValue([{ id:"pending",source_type:"zotero",path:"zotero/private-locator",space:"Research",status:"Active",last_sync:null,file_count:0,memory_count:0,last_sync_error_detail:"zotero_disconnect_pending" }]);
    vi.mocked(tauri.getZoteroDisconnectPreview).mockResolvedValue({identity:{registration_id:"pending",locator:"zotero/private-locator",server_id:"server",space_id:"space"},identity_digest:"digest",space_name:"Research",revision:1,canonical_item_ids:[],affected_page_ids:[],impact_scope_known:false,pending_disconnect:true,disconnected:false});
    render(<SourcesSection />, {wrapper});
    expect(await screen.findByText("Zotero · Research")).toBeInTheDocument();
    expect(screen.queryByText(/0 files/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", {name:"Sync"})).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button",{name:"Finish disconnect"}));
    expect(await screen.findByText(/previous disconnect was interrupted/)).toBeInTheDocument();
    expect(tauri.getZoteroDisconnectPreview).toHaveBeenCalledWith("pending");
  });
});
