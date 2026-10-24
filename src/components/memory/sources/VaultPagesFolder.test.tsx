// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getKnowledgePathInfo, restartDaemonToMovePages, setKnowledgePath, type RegisteredSource } from "../../../lib/tauri";
import { VaultPagesFolder, refusalKey, shortPath, shownFromVault } from "./VaultPagesFolder";

vi.mock("../../../lib/tauri", () => ({
  getKnowledgePathInfo: vi.fn(),
  setKnowledgePath: vi.fn(),
  restartDaemonToMovePages: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

const vault: RegisteredSource = {
  id: "vault-1", source_type: "obsidian", path: "/Users/me/My notes", status: "Active",
  last_sync: 1_728_000_000, file_count: 12, memory_count: 8,
};

function renderFolder() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><VaultPagesFolder source={vault} /></QueryClientProvider>);
}

/** The folder shown in the panel, read across its wrap points. */
function shownPaths() {
  return Array.from(document.querySelectorAll(".vault-pages-path")).map((node) => node.textContent);
}

beforeEach(() => {
  vi.resetAllMocks();
});

describe("VaultPagesFolder", () => {
  it("names a folder inside the vault from the vault", () => {
    expect(shownFromVault("/Users/me/My notes/Wenlan", "/Users/me/My notes/")).toBe("My notes/Wenlan");
    expect(shownFromVault("/Users/me/My notes-2/Wenlan", "/Users/me/My notes")).toBeNull();
    expect(shownFromVault("C:\\Vault\\Wenlan", "C:\\Vault")).toBe("Vault/Wenlan");
  });

  it("shortens a home folder to ~", () => {
    expect(shortPath("/Users/me/.wenlan/pages")).toBe("~/.wenlan/pages");
    expect(shortPath("/home/me/notes/")).toBe("~/notes");
    expect(shortPath("C:\\Users\\me\\Wenlan")).toBe("~/Wenlan");
    expect(shortPath("/Volumes/Notes/Wenlan")).toBe("/Volumes/Notes/Wenlan");
  });

  it("turns refusal codes into words", () => {
    expect(refusalKey("HTTP POST /api/knowledge/path returned 400: pages_folder_not_empty")).toBe("sourceContainer.pagesNotEmpty");
    expect(refusalKey("pages_folder_in_app_folder")).toBe("sourceContainer.pagesAppFolder");
    expect(refusalKey("something else")).toBe("sourceContainer.pagesBadFolder");
  });

  it("moves the pages into the vault after the person confirms", async () => {
    let path = "/Users/me/.wenlan/pages";
    vi.mocked(getKnowledgePathInfo).mockImplementation(async () => ({ path }));
    vi.mocked(setKnowledgePath).mockResolvedValue({ path, pending_path: "/Users/me/My notes/Wenlan" });
    vi.mocked(restartDaemonToMovePages).mockImplementation(async () => { path = "/Users/me/My notes/Wenlan"; return "0.20.1"; });
    renderFolder();

    expect(await screen.findByText("Move them into this vault to read and edit them in Obsidian.")).toBeInTheDocument();
    expect(shownPaths()).toEqual(["~/.wenlan/pages"]);
    fireEvent.click(await screen.findByRole("button", { name: "Move into this vault" }));
    expect(screen.getByText("Move Wenlan's pages to this folder?")).toBeInTheDocument();
    expect(shownPaths()).toEqual(["My notes/Wenlan"]);
    expect(setKnowledgePath).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(await screen.findByText("Pages moved.")).toBeInTheDocument();
    expect(setKnowledgePath).toHaveBeenCalledWith("/Users/me/My notes/Wenlan");
    expect(shownPaths()).toEqual(["My notes/Wenlan"]);
    expect(screen.getByRole("button", { name: "Change" })).toBeInTheDocument();
  });

  it("cancel moves nothing", async () => {
    vi.mocked(getKnowledgePathInfo).mockResolvedValue({ path: "/Users/me/.wenlan/pages" });
    renderFolder();
    fireEvent.click(await screen.findByRole("button", { name: "Move into this vault" }));
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(setKnowledgePath).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Move into this vault" })).toBeInTheDocument();
  });

  it("asks for a restart by hand when the app cannot restart the service", async () => {
    let pending: string | undefined;
    vi.mocked(getKnowledgePathInfo).mockImplementation(async () => ({ path: "/Users/me/.wenlan/pages", pending_path: pending }));
    vi.mocked(setKnowledgePath).mockImplementation(async (folder) => { pending = folder; return { path: "/Users/me/.wenlan/pages", pending_path: folder }; });
    vi.mocked(restartDaemonToMovePages).mockRejectedValue("daemon-restart:isolated");
    renderFolder();
    fireEvent.click(await screen.findByRole("button", { name: "Move into this vault" }));
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(await screen.findByText("Reopen Wenlan to finish the move.")).toBeInTheDocument();
    // Waiting: the new folder shows and there is nothing to press again.
    expect(shownPaths()).toEqual(["My notes/Wenlan"]);
    expect(screen.queryByRole("button", { name: "Move into this vault" })).toBeNull();
  });

  it("explains a refused folder and lets the person pick another", async () => {
    vi.mocked(getKnowledgePathInfo).mockResolvedValue({ path: "/Users/me/.wenlan/pages" });
    vi.mocked(setKnowledgePath).mockRejectedValue("HTTP POST /api/knowledge/path returned 400: pages_folder_not_empty");
    vi.mocked(openDialog).mockResolvedValue("/Users/me/My notes/Wenlan pages");
    renderFolder();
    fireEvent.click(await screen.findByRole("button", { name: "Move into this vault" }));
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("That folder already has files.");
    expect(restartDaemonToMovePages).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole("button", { name: "Change" }));
    expect(await screen.findByText("Move Wenlan's pages to this folder?")).toBeInTheDocument();
    expect(shownPaths()).toEqual(["My notes/Wenlan pages"]);
    expect(openDialog).toHaveBeenCalledWith(expect.objectContaining({ directory: true, defaultPath: "/Users/me/My notes" }));
  });

  it("says a move is waiting for a restart", async () => {
    vi.mocked(getKnowledgePathInfo).mockResolvedValue({ path: "/Users/me/.wenlan/pages", pending_path: "/Users/me/My notes/Wenlan" });
    renderFolder();
    expect(await screen.findByText("Reopen Wenlan to finish the move.")).toBeInTheDocument();
    expect(shownPaths()).toEqual(["My notes/Wenlan"]);
    expect(screen.queryByRole("button")).toBeNull();
  });
});
