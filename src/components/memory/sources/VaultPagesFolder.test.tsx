// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { getKnowledgePathInfo, restartDaemonToMovePages, setKnowledgePath, type RegisteredSource } from "../../../lib/tauri";
import { VaultPagesFolder, refusalKey, shownFromVault } from "./VaultPagesFolder";

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

beforeEach(() => {
  vi.resetAllMocks();
});

describe("VaultPagesFolder", () => {
  it("names a folder inside the vault from the vault", () => {
    expect(shownFromVault("/Users/me/My notes/Wenlan", "/Users/me/My notes/")).toBe("My notes/Wenlan");
    expect(shownFromVault("/Users/me/My notes-2/Wenlan", "/Users/me/My notes")).toBeNull();
    expect(shownFromVault("C:\\Vault\\Wenlan", "C:\\Vault")).toBe("Vault/Wenlan");
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

    expect(await screen.findByText("/Users/me/.wenlan/pages")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Move into this vault" }));
    expect(screen.getByText(/Move Wenlan's pages into My notes\/Wenlan\?/)).toBeInTheDocument();
    expect(setKnowledgePath).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(await screen.findByText("Pages moved to My notes/Wenlan.")).toBeInTheDocument();
    expect(setKnowledgePath).toHaveBeenCalledWith("/Users/me/My notes/Wenlan");
    expect(await screen.findByText("My notes/Wenlan")).toBeInTheDocument();
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
    vi.mocked(getKnowledgePathInfo).mockResolvedValue({ path: "/Users/me/.wenlan/pages" });
    vi.mocked(setKnowledgePath).mockResolvedValue({ path: "/Users/me/.wenlan/pages", pending_path: "/Users/me/My notes/Wenlan" });
    vi.mocked(restartDaemonToMovePages).mockRejectedValue("daemon-restart:isolated");
    renderFolder();
    fireEvent.click(await screen.findByRole("button", { name: "Move into this vault" }));
    fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(await screen.findByText("Quit and reopen Wenlan to finish moving the pages.")).toBeInTheDocument();
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
    expect(await screen.findByText(/Move Wenlan's pages into My notes\/Wenlan pages\?/)).toBeInTheDocument();
    expect(openDialog).toHaveBeenCalledWith(expect.objectContaining({ directory: true, defaultPath: "/Users/me/My notes" }));
  });

  it("says a move is waiting for a restart", async () => {
    vi.mocked(getKnowledgePathInfo).mockResolvedValue({ path: "/Users/me/.wenlan/pages", pending_path: "/Users/me/My notes/Wenlan" });
    renderFolder();
    expect(await screen.findByText("They move to My notes/Wenlan when Wenlan restarts.")).toBeInTheDocument();
  });
});
