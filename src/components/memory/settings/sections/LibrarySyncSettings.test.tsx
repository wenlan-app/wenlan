// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open } from "@tauri-apps/plugin-dialog";
import { i18n } from "../../../../i18n";
import {
  listShares,
  lookInSharedFolder,
  setLibraryMainDevice,
  shareLibrary,
  stopSharingLibrary,
  suggestLibraryFolder,
  type SharedStatus,
} from "../../../../lib/tauri";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import LibrarySyncSettings from "./LibrarySyncSettings";

vi.mock("../../../../lib/tauri", () => ({
  listShares: vi.fn(),
  lookInSharedFolder: vi.fn(),
  revealSharedFolder: vi.fn(() => Promise.resolve()),
  shareLibrary: vi.fn(() => Promise.resolve({ exported: 0, applied: 0, conflicts: 0, skipped: 0 })),
  stopSharingLibrary: vi.fn(() => Promise.resolve()),
  setLibraryMainDevice: vi.fn(() => Promise.resolve({})),
  suggestLibraryFolder: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn(() => Promise.resolve()) }));

const FOLDER = "/Users/me/Library/CloudStorage/GoogleDrive-me@example.com/My Drive/Wenlan";

const on: SharedStatus = {
  kind: "library",
  target: "library",
  folder: FOLDER,
  member_id: "me",
  member_name: "Me",
  members: [{ id: "me", name: "Me" }],
  last: { at: new Date(Date.now() - 120_000).toISOString(), counts: { exported: 0, applied: 0, conflicts: 0, skipped: 0 } },
  devices: [{ id: "d1", name: "MacBook" }, { id: "d2", name: "Windows PC" }],
  main_device: "d1",
  this_device: "d1",
};

function renderRow() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <LibrarySyncSettings />
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value(this: HTMLDialogElement) { this.open = true; } });
  Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, value(this: HTMLDialogElement) { this.open = false; } });
  await i18n.changeLanguage("en");
  vi.mocked(listShares).mockResolvedValue([]);
  vi.mocked(suggestLibraryFolder).mockResolvedValue({ folder: null, cloud: null });
  localStorage.clear();
});
afterEach(() => vi.clearAllMocks());

describe("library sync row", () => {
  it("is off by default and starts syncing with the chosen folder", async () => {
    vi.mocked(open).mockResolvedValue(FOLDER);
    vi.mocked(lookInSharedFolder).mockResolvedValue(null);
    renderRow();
    expect(await screen.findByText("Uses Google Drive or iCloud")).toBeInTheDocument();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    expect(screen.getByText("Won't sync")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Choose…" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "Start syncing" }));
    await waitFor(() => expect(shareLibrary).toHaveBeenCalledWith(FOLDER, null));
  });

  it("says phone AI needs Google Drive when the folder is in iCloud", async () => {
    vi.mocked(open).mockResolvedValue("/Users/me/Library/Mobile Documents/com~apple~CloudDocs/Wenlan");
    vi.mocked(lookInSharedFolder).mockResolvedValue(null);
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    expect(screen.getByText("Reads pages with computers off")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Choose…" }));
    expect(await screen.findByText("Needs Google Drive")).toBeInTheDocument();
  });

  it("merges into a library the folder already holds, as that member", async () => {
    vi.mocked(open).mockResolvedValue(FOLDER);
    vi.mocked(lookInSharedFolder).mockResolvedValue({ kind: "library", name: "library", members: [{ id: "m-1", name: "Me" }] });
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    await userEvent.click(screen.getByRole("button", { name: "Choose…" }));
    expect(await screen.findByText("Your other computer's Wenlan")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Merge" }));
    await waitFor(() => expect(shareLibrary).toHaveBeenCalledWith(FOLDER, "m-1"));
  });

  it("refuses a folder that holds a Space share", async () => {
    vi.mocked(open).mockResolvedValue(FOLDER);
    vi.mocked(lookInSharedFolder).mockResolvedValue({ kind: "space", name: "Wedding", members: [] });
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    await userEvent.click(screen.getByRole("button", { name: "Choose…" }));
    expect(await screen.findByRole("alert")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled();
  });

  it("lists the computers on one line and changes the main one", async () => {
    vi.mocked(listShares).mockResolvedValue([on]);
    renderRow();
    expect(await screen.findByText("Google Drive · synced 2 min. ago")).toBeInTheDocument();
    expect(screen.getByText("MacBook (this one, main), Windows PC")).toBeInTheDocument();
    expect(screen.queryByText("Main computer")).not.toBeInTheDocument();
    expect(screen.getByText("Google Drive › Wenlan")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Change main" }));
    await userEvent.selectOptions(screen.getByRole("combobox", { name: "Choose the main computer" }), "d2");
    await waitFor(() => expect(setLibraryMainDevice).toHaveBeenCalledWith("d2"));
  });

  it("asks before it stops syncing", async () => {
    vi.mocked(listShares).mockResolvedValue([on]);
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    expect(screen.getByText("Stop syncing? Your pages stay.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Stop syncing" }));
    await waitFor(() => expect(stopSharingLibrary).toHaveBeenCalledOnce());
  });

  it("picks this computer's Google Drive by itself", async () => {
    vi.mocked(suggestLibraryFolder).mockResolvedValue({ folder: FOLDER, cloud: "google_drive" });
    vi.mocked(lookInSharedFolder).mockResolvedValue(null);
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    expect(await screen.findByText("Your Google Drive, picked for you")).toBeInTheDocument();
    expect(screen.getByText("Google Drive › Wenlan")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change…" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Start syncing" }));
    await waitFor(() => expect(shareLibrary).toHaveBeenCalledWith(FOLDER, null));
  });

  it("offers to merge at once when the other computer's library is in the Drive", async () => {
    vi.mocked(suggestLibraryFolder).mockResolvedValue({ folder: FOLDER, cloud: "google_drive" });
    vi.mocked(lookInSharedFolder).mockResolvedValue({ kind: "library", name: "library", members: [{ id: "m-1", name: "Me" }] });
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    await userEvent.click(await screen.findByRole("button", { name: "Merge" }));
    await waitFor(() => expect(shareLibrary).toHaveBeenCalledWith(FOLDER, "m-1"));
  });

  it("says how to get Google Drive when this computer has none", async () => {
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    expect(await screen.findByText(/No Google Drive on this computer/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Get Google Drive" }));
    expect(openExternal).toHaveBeenCalledWith("https://www.google.com/drive/download/");
  });

  it("shows how far a running pass has come", async () => {
    vi.mocked(listShares).mockResolvedValue([{ ...on, progress: { done: 120, total: 480 } }]);
    renderRow();
    expect(await screen.findByText("Google Drive · syncing 120 of 480 pages")).toBeInTheDocument();
  });

  it("lists the next steps until the person closes them", async () => {
    vi.mocked(listShares).mockResolvedValue([{ ...on, devices: [on.devices![0]] }]);
    const { unmount } = renderRow();
    expect(await screen.findByText("Open Settings › Sync and start")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "How" }));
    expect(screen.getByText("Let your phone's AI read your pages")).toBeInTheDocument();
    expect(screen.getByText("Tell it: “Read Wenlan › 給 AI 讀的頁面 in my Google Drive.”")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Got it" }));
    unmount();

    // The other computer joined: ticked by itself.
    vi.mocked(listShares).mockResolvedValue([on]);
    renderRow();
    expect(await screen.findByText("Windows PC is connected")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByText("Next steps")).not.toBeInTheDocument();
  });

  it("shows a held-back run as a warning", async () => {
    vi.mocked(listShares).mockResolvedValue([{ ...on, last: { ...on.last!, held_back: "x" } }]);
    renderRow();
    expect(await screen.findByText("Another computer archived many pages. Kept here.")).toBeInTheDocument();
  });
});
