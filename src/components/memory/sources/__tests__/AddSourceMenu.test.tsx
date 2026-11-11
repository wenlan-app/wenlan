import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { vi, describe, it, expect, beforeEach } from "vitest";
import AddSourceMenu from "../AddSourceMenu";

const mocks = vi.hoisted(() => ({
  getDaemonVersion: vi.fn(),
  uploadSourceFile: vi.fn(),
  openFile: vi.fn(),
  openDialog: vi.fn(),
  addSource: vi.fn(),
  syncRegisteredSource: vi.fn(),
  detectVault: vi.fn(),
  listZoteroCollections: vi.fn(),
  listSpaces: vi.fn(),
  detectObsidianVaults: vi.fn(),
  linkObsidianVault: vi.fn(),
}));

vi.mock("../../../../lib/tauri", () => ({
  getDaemonVersion: mocks.getDaemonVersion,
  daemonMeetsFloor: (v: string) => v.split(".").map(Number)[1] >= 10,
  uploadSourceFile: mocks.uploadSourceFile,
  openFile: mocks.openFile,
  addSource: mocks.addSource,
  syncRegisteredSource: mocks.syncRegisteredSource,
  listZoteroCollections: mocks.listZoteroCollections,
  listSpaces: mocks.listSpaces,
  detectObsidianVaults: mocks.detectObsidianVaults,
  linkObsidianVault: mocks.linkObsidianVault,
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: mocks.openDialog }));
vi.mock("sonner", () => ({ toast: vi.fn() }));
vi.mock("../../../../lib/vaultDetection", () => ({ detectVault: mocks.detectVault }));

function wrap(ui: React.ReactNode) {
  return <QueryClientProvider client={new QueryClient()}>{ui}</QueryClientProvider>;
}

describe("AddSourceMenu", () => {
  beforeEach(() => {
    vi.resetAllMocks();
    mocks.getDaemonVersion.mockResolvedValue("0.11.0");
    mocks.detectVault.mockResolvedValue({ isVault: false, sourceType: "directory", unreadable: false });
    mocks.addSource.mockResolvedValue({ id: "connected", path: "/Users/me/research", source_type: "directory" });
    mocks.syncRegisteredSource.mockResolvedValue({ files_found: 1, ingested: 1, skipped: 0, errors: 0 });
    mocks.detectObsidianVaults.mockResolvedValue([]);
    mocks.linkObsidianVault.mockResolvedValue({ vaults: [{ path: "/Users/me/vault", folder: "vault" }], notes_added: 2 });
  });

  it("offers four ways in by what you have, with saving a link as a small link under them", async () => {
    mocks.getDaemonVersion.mockResolvedValue("0.11.0");
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    await screen.findByText("Folder");
    const options = screen.getAllByRole("button").filter((button) => button.classList.contains("source-add-option"));
    expect(options.map((option) => option.querySelector("strong")?.textContent)).toEqual(["Past AI chats", "Folder", "Files", "Zotero"]);
    expect(screen.queryByText("Obsidian vault")).toBeNull();
    expect(screen.queryByText("Logseq graph")).toBeNull();
    fireEvent.click(screen.getByTestId("source-add-link"));
    expect(await screen.findByRole("dialog", { name: "Add a link with text" })).toBeInTheDocument();
    expect(screen.queryByText("Your daemon needs an update to index files.")).toBeNull();
  });

  it("offers Zotero next to the other sources and opens its import in place", async () => {
    mocks.listZoteroCollections.mockResolvedValue({ server_id: "local", collections: [{ key: "A1", name: "Articles", item_count: 2 }] });
    mocks.listSpaces.mockResolvedValue([]);
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Zotero"));
    expect(await screen.findByRole("dialog", { name: "Import from Zotero" })).toBeInTheDocument();
    expect(await screen.findByText("Connected to local Zotero")).toBeInTheDocument();
  });

  it("offers past AI chats and opens the chat export import in place", async () => {
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Past AI chats"));
    expect(await screen.findByRole("dialog", { name: "Past AI chats" })).toBeInTheDocument();
    expect(screen.getByTestId("chat-import-drop-zone")).toBeInTheDocument();
  });

  it("puts the quick paste first, with the full export (which can take a day) as a link under it", async () => {
    render(wrap(<AddSourceMenu start="chats" onQuickChats={vi.fn()} onClose={() => {}} />));
    expect(await screen.findByRole("dialog", { name: "Past AI chats" })).toBeInTheDocument();
    expect(screen.getByTestId("source-chats-quick")).toHaveTextContent("Paste what your AI remembers about you");
    expect(screen.queryByTestId("chat-import-drop-zone")).toBeNull();
    fireEvent.click(screen.getByTestId("source-chats-full"));
    expect(screen.getByTestId("chat-import-drop-zone")).toBeInTheDocument();
    expect(screen.queryByTestId("source-chats-quick")).toBeNull();
  });

  it("hands the quick way to the paste-back import and closes", async () => {
    const onClose = vi.fn();
    const onQuickChats = vi.fn();
    render(wrap(<AddSourceMenu start="chats" onQuickChats={onQuickChats} onClose={onClose} />));
    fireEvent.click(await screen.findByTestId("source-chats-quick"));
    expect(onClose).toHaveBeenCalledTimes(1);
    expect(onQuickChats).toHaveBeenCalledTimes(1);
  });

  it("shows the update-daemon notice on an old daemon", async () => {
    mocks.getDaemonVersion.mockResolvedValue("0.9.5");
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    await screen.findByText("Your daemon needs an update to index files.");
  });

  it("picking Add files stages the chosen file", async () => {
    mocks.getDaemonVersion.mockResolvedValue("0.11.0");
    mocks.openDialog.mockResolvedValue("/Users/me/paper.pdf");
    mocks.uploadSourceFile.mockResolvedValue({ id: "directory-sources" });
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Files"));
    await waitFor(() => expect(mocks.uploadSourceFile).toHaveBeenCalledWith("/Users/me/paper.pdf"));
  });
  it("opened from the welcome's folder row, goes straight to the folder picker once", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/vault");
    render(wrap(<AddSourceMenu start="folder" onClose={() => {}} />));
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("directory", "/Users/me/vault", true));
    expect(mocks.openDialog).toHaveBeenCalledOnce();
    expect(mocks.openDialog).toHaveBeenCalledWith({ title: "Folder", directory: true, multiple: false });
  });

  it("a cancelled picker from the welcome leaves the menu with every other way in", async () => {
    mocks.openDialog.mockResolvedValue(null);
    render(wrap(<AddSourceMenu start="folder" onClose={() => {}} />));
    await waitFor(() => expect(mocks.openDialog).toHaveBeenCalledOnce());
    expect(await screen.findByText("Past AI chats")).toBeInTheDocument();
  });

  it("opens the system folder picker directly and keeps the folder in sync without asking", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/research");
    const close = vi.fn();
    render(wrap(<AddSourceMenu onClose={close} />));
    fireEvent.click(await screen.findByText("Folder"));
    expect(screen.queryByRole("dialog", { name: "Sync or import once?" })).toBeNull();
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("directory", "/Users/me/research", true));
    expect(mocks.openDialog).toHaveBeenCalledWith({ title: "Folder", directory: true, multiple: false });
    expect(mocks.detectVault).toHaveBeenCalledWith("/Users/me/research");
    await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
    expect(mocks.syncRegisteredSource).toHaveBeenCalledWith("connected");
  });

  it("does not register a source when the native folder picker is cancelled", async () => {
    mocks.openDialog.mockResolvedValue(null);
    const close = vi.fn();
    render(wrap(<AddSourceMenu onClose={close} />));
    fireEvent.click(await screen.findByText("Folder"));
    await waitFor(() => expect(screen.queryByRole("status")).toBeNull());
    expect(mocks.detectVault).not.toHaveBeenCalled();
    expect(mocks.addSource).not.toHaveBeenCalled();
    expect(close).not.toHaveBeenCalled();
  });

  it("a vault picked from the folder entry is linked into the Wiki, with that vault already chosen (UX-ADD-3)", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/vault");
    mocks.detectVault.mockResolvedValue({ isVault: true, sourceType: "obsidian", unreadable: false });
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Folder"));
    expect(await screen.findByRole("dialog", { name: "Link an Obsidian vault" })).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Link" }));
    await waitFor(() => expect(mocks.linkObsidianVault).toHaveBeenCalledWith("/Users/me/vault"));
    expect(mocks.addSource).not.toHaveBeenCalled();
  });

  it("recognises a Logseq graph picked from the folder entry and connects it with its own source type", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/graph");
    mocks.detectVault.mockResolvedValue({ isVault: false, isLogseqGraph: true, sourceType: "directory", unreadable: false });
    mocks.addSource.mockResolvedValue({ id: "logseq-graph", path: "/Users/me/graph", source_type: "logseq" });
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Folder"));
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("logseq", "/Users/me/graph", true));
    await waitFor(() => expect(mocks.syncRegisteredSource).toHaveBeenCalledWith("logseq-graph"));
  });

  it("links a folder that is both a Logseq graph and an Obsidian vault as the vault", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/graph");
    mocks.detectVault.mockResolvedValue({ isVault: true, isLogseqGraph: true, sourceType: "obsidian", unreadable: false });
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Folder"));
    expect(await screen.findByRole("dialog", { name: "Link an Obsidian vault" })).toBeInTheDocument();
    expect(mocks.addSource).not.toHaveBeenCalled();
  });

  it("keeps the connection when the first read fails and reports where to retry", async () => {
    const { toast } = await import("sonner");
    mocks.openDialog.mockResolvedValue("/Users/me/research");
    mocks.syncRegisteredSource.mockRejectedValue(new Error("offline"));
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Folder"));
    await waitFor(() => expect(toast).toHaveBeenLastCalledWith("Source connected", {
      id: expect.stringMatching(/^source-connected-/),
      description: "First update failed. Try Refresh in Info.",
    }));
    expect(mocks.addSource).toHaveBeenCalledTimes(1);
  });

});
