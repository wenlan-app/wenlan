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
}));

vi.mock("../../../../lib/tauri", () => ({
  getDaemonVersion: mocks.getDaemonVersion,
  daemonMeetsFloor: (v: string) => v.split(".").map(Number)[1] >= 10,
  uploadSourceFile: mocks.uploadSourceFile,
  openFile: mocks.openFile,
  addSource: mocks.addSource,
  syncRegisteredSource: mocks.syncRegisteredSource,
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
  });

  it("shows file, folder and web excerpt entry points on a current daemon", async () => {
    mocks.getDaemonVersion.mockResolvedValue("0.11.0");
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    await screen.findByText("Folder");
    expect(screen.getByText("Files")).toBeInTheDocument();
    expect(screen.getByText("Link and text")).toBeInTheDocument();
    expect(screen.queryByText("Your daemon needs an update to index files.")).toBeNull();
  });

  it("offers past AI chats and opens the chat export import in place", async () => {
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Past AI chats"));
    expect(await screen.findByRole("dialog", { name: "Past AI chats" })).toBeInTheDocument();
    expect(screen.getByTestId("chat-import-drop-zone")).toBeInTheDocument();
  });

  it("offers the quick and the full way for past AI chats side by side where both exist", async () => {
    const onClose = vi.fn();
    const onQuickChats = vi.fn();
    render(wrap(<AddSourceMenu start="chats" onQuickChats={onQuickChats} onClose={onClose} />));
    expect(await screen.findByRole("dialog", { name: "Past AI chats" })).toBeInTheDocument();
    expect(screen.getByTestId("source-chats-quick")).toHaveTextContent("Quick, about a minute");
    expect(screen.getByTestId("source-chats-full")).toHaveTextContent("Full chat history");
    expect(screen.queryByTestId("chat-import-drop-zone")).toBeNull();

    fireEvent.click(screen.getByTestId("source-chats-full"));
    expect(screen.getByTestId("chat-import-drop-zone")).toBeInTheDocument();
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
  it("opens the system folder picker directly and connects the selected folder", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/research");
    const close = vi.fn();
    render(wrap(<AddSourceMenu onClose={close} />));
    fireEvent.click(await screen.findByText("Folder"));
    expect(await screen.findByRole("dialog", { name: "Sync or import once?" })).toBeInTheDocument();
    expect(screen.getByText("/Users/me/research")).toBeInTheDocument();
    expect(mocks.addSource).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText("Keep in sync"));
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("directory", "/Users/me/research", true));
    expect(mocks.openDialog).toHaveBeenCalledWith({ title: "Folder", directory: true, multiple: false });
    expect(mocks.detectVault).toHaveBeenCalledWith("/Users/me/research");
    await waitFor(() => expect(close).toHaveBeenCalledTimes(1));
    expect(mocks.syncRegisteredSource).toHaveBeenCalledWith("connected");
  });

  it("lists past AI chats first, then Obsidian, and connects a vault with its own source type", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/vault");
    mocks.detectVault.mockResolvedValue({ isVault: true, sourceType: "obsidian", unreadable: false });
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    const vault = await screen.findByText("Obsidian vault");
    const options = screen.getAllByRole("button").filter((button) => button.classList.contains("source-add-option"));
    expect(options[0]).toHaveTextContent("Past AI chats");
    expect(options[1]).toBe(vault.closest("button"));
    fireEvent.click(vault);
    fireEvent.click(await screen.findByText("Keep in sync"));
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("obsidian", "/Users/me/vault", true));
    expect(mocks.openDialog).toHaveBeenCalledWith({ title: "Obsidian vault", directory: true, multiple: false });
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

  it("does not label a normal folder as an Obsidian vault", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/research");
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Obsidian vault"));
    // Shown in the dialog, next to the choice that failed.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not add the source");
    expect(alert).toHaveTextContent("Choose the root folder of an Obsidian vault.");
    expect(mocks.addSource).not.toHaveBeenCalled();
  });

  it("detects Obsidian even when selected from the ordinary folder entry", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/vault");
    mocks.detectVault.mockResolvedValue({ isVault: true, sourceType: "obsidian", unreadable: false });
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Folder"));
    fireEvent.click(await screen.findByText("Keep in sync"));
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("obsidian", "/Users/me/vault", true));
  });

  it("imports a vault once: registers it without following changes and still reads it now", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/vault");
    mocks.detectVault.mockResolvedValue({ isVault: true, sourceType: "obsidian", unreadable: false });
    mocks.addSource.mockResolvedValue({ id: "obsidian-vault", path: "/Users/me/vault", source_type: "obsidian", status: "Paused" });
    const close = vi.fn();
    render(wrap(<AddSourceMenu onClose={close} />));
    fireEvent.click(await screen.findByText("Obsidian vault"));
    fireEvent.click(await screen.findByText("Import once"));
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("obsidian", "/Users/me/vault", false));
    await waitFor(() => expect(mocks.syncRegisteredSource).toHaveBeenCalledWith("obsidian-vault"));
    expect(close).toHaveBeenCalledTimes(1);
  });

  it("closing the sync choice registers nothing", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/research");
    const close = vi.fn();
    render(wrap(<AddSourceMenu onClose={close} />));
    fireEvent.click(await screen.findByText("Folder"));
    await screen.findByRole("dialog", { name: "Sync or import once?" });
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    expect(close).toHaveBeenCalledTimes(1);
    expect(mocks.addSource).not.toHaveBeenCalled();
  });

  it("connects a Logseq graph with its own source type, even one Obsidian also opens", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/graph");
    mocks.detectVault.mockResolvedValue({ isVault: true, isLogseqGraph: true, sourceType: "obsidian", unreadable: false });
    mocks.addSource.mockResolvedValue({ id: "logseq-graph", path: "/Users/me/graph", source_type: "logseq" });
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Logseq graph"));
    fireEvent.click(await screen.findByText("Keep in sync"));
    await waitFor(() => expect(mocks.addSource).toHaveBeenCalledWith("logseq", "/Users/me/graph", true));
    expect(mocks.openDialog).toHaveBeenCalledWith({ title: "Logseq graph", directory: true, multiple: false });
    await waitFor(() => expect(mocks.syncRegisteredSource).toHaveBeenCalledWith("logseq-graph"));
  });

  it("does not label a normal folder as a Logseq graph", async () => {
    mocks.openDialog.mockResolvedValue("/Users/me/research");
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Logseq graph"));
    // Shown in the dialog, next to the choice that failed.
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Could not add the source");
    expect(alert).toHaveTextContent("Choose the root folder of a Logseq graph.");
    expect(mocks.addSource).not.toHaveBeenCalled();
  });

  it("keeps the connection when the first read fails and reports where to retry", async () => {
    const { toast } = await import("sonner");
    mocks.openDialog.mockResolvedValue("/Users/me/research");
    mocks.syncRegisteredSource.mockRejectedValue(new Error("offline"));
    render(wrap(<AddSourceMenu onClose={() => {}} />));
    fireEvent.click(await screen.findByText("Folder"));
    fireEvent.click(await screen.findByText("Import once"));
    await waitFor(() => expect(toast).toHaveBeenLastCalledWith("Source connected", {
      id: expect.stringMatching(/^source-connected-/),
      description: "First update failed. Try Refresh in Info.",
    }));
    expect(mocks.addSource).toHaveBeenCalledTimes(1);
  });

});
