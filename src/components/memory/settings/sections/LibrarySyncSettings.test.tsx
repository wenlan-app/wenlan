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
  type SharedStatus,
} from "../../../../lib/tauri";
import LibrarySyncSettings from "./LibrarySyncSettings";

vi.mock("../../../../lib/tauri", () => ({
  listShares: vi.fn(),
  lookInSharedFolder: vi.fn(),
  revealSharedFolder: vi.fn(() => Promise.resolve()),
  shareLibrary: vi.fn(() => Promise.resolve({ exported: 0, applied: 0, conflicts: 0, skipped: 0 })),
  stopSharingLibrary: vi.fn(() => Promise.resolve()),
  setLibraryMainDevice: vi.fn(() => Promise.resolve({})),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));

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
    // An empty folder: asked which computer this is before anything starts.
    await screen.findByRole("radio", { name: "The first one" });
    expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled();
    await userEvent.click(screen.getByRole("radio", { name: "The first one" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Start syncing" })).toBeEnabled());
    await userEvent.click(screen.getByRole("button", { name: "Start syncing" }));
    await waitFor(() => expect(shareLibrary).toHaveBeenCalledWith(FOLDER, null));
  });

  it("never starts a second library while the other computer's files are still downloading", async () => {
    vi.mocked(open).mockResolvedValue(FOLDER);
    vi.mocked(lookInSharedFolder).mockResolvedValue(null);
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sync to other computers and phone" }));
    await userEvent.click(screen.getByRole("button", { name: "Choose…" }));
    await userEvent.click(await screen.findByRole("radio", { name: "Another one" }));
    expect(screen.getByText(/Nothing from your other computer yet/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start syncing" })).toBeDisabled();
    // The files arrive: checking again finds the library, and it merges.
    vi.mocked(lookInSharedFolder).mockResolvedValue({ kind: "library", name: "Wenlan", page_id: null, members: [{ id: "m1", name: "Lucian" }] });
    await userEvent.click(screen.getByRole("button", { name: "Check again" }));
    await userEvent.click(await screen.findByRole("button", { name: "Merge" }));
    await waitFor(() => expect(shareLibrary).toHaveBeenCalledWith(FOLDER, "m1"));
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

  it("lists the computers and changes the main one", async () => {
    vi.mocked(listShares).mockResolvedValue([on]);
    renderRow();
    expect(await screen.findByText("Google Drive · synced 2 min. ago")).toBeInTheDocument();
    expect(screen.getByText("MacBook (this one), Windows PC")).toBeInTheDocument();
    expect(screen.getByText("Runs AI organizing and phone pages")).toBeInTheDocument();
    expect(screen.getByText("Google Drive › Wenlan")).toBeInTheDocument();
    const changes = screen.getAllByRole("button", { name: "Change" });
    await userEvent.click(changes[0]);
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

  it("shows a held-back run as a warning", async () => {
    vi.mocked(listShares).mockResolvedValue([{ ...on, last: { ...on.last!, held_back: "x" } }]);
    renderRow();
    expect(await screen.findByText("Another computer archived many pages. Kept here.")).toBeInTheDocument();
  });
});
