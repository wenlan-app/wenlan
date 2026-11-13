// SPDX-License-Identifier: AGPL-3.0-only
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { SpaceFoldersPrompt } from "./SpaceFoldersPrompt";

const getSpaceFolders = vi.fn<() => Promise<boolean | null>>();
const setSpaceFolders = vi.fn<(on: boolean) => Promise<void>>();
vi.mock("../../../lib/tauri", () => ({
  getSpaceFolders: () => getSpaceFolders(),
  setSpaceFolders: (on: boolean) => setSpaceFolders(on),
}));

function renderPrompt(noteCount: number | null) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><SpaceFoldersPrompt noteCount={noteCount} /></QueryClientProvider>);
}

describe("SpaceFoldersPrompt", () => {
  beforeEach(() => {
    getSpaceFolders.mockReset();
    setSpaceFolders.mockReset().mockResolvedValue(undefined);
  });

  it("asks once when the library already has pages, and says yes", async () => {
    getSpaceFolders.mockResolvedValue(null);
    renderPrompt(12);
    fireEvent.click(await screen.findByRole("button", { name: "Yes" }));
    await waitFor(() => expect(setSpaceFolders).toHaveBeenCalledWith(true));
    await waitFor(() => expect(screen.queryByRole("group")).toBeNull());
  });

  it("can be turned down", async () => {
    getSpaceFolders.mockResolvedValue(null);
    renderPrompt(3);
    fireEvent.click(await screen.findByRole("button", { name: "Not now" }));
    await waitFor(() => expect(setSpaceFolders).toHaveBeenCalledWith(false));
  });

  it("never turns itself on, even when the list in view is empty", async () => {
    getSpaceFolders.mockResolvedValue(null);
    renderPrompt(0);
    await waitFor(() => expect(getSpaceFolders).toHaveBeenCalled());
    await new Promise(resolve => setTimeout(resolve, 20));
    expect(setSpaceFolders).not.toHaveBeenCalled();
    expect(screen.queryByRole("group")).toBeNull();
  });

  it("does not ask again once answered, or when the daemon cannot say", async () => {
    getSpaceFolders.mockResolvedValue(false);
    const { unmount } = renderPrompt(5);
    await waitFor(() => expect(getSpaceFolders).toHaveBeenCalled());
    expect(screen.queryByRole("group")).toBeNull();
    unmount();
    getSpaceFolders.mockRejectedValue(new Error("old daemon"));
    renderPrompt(5);
    await waitFor(() => expect(getSpaceFolders).toHaveBeenCalledTimes(2));
    expect(screen.queryByRole("group")).toBeNull();
    expect(setSpaceFolders).not.toHaveBeenCalled();
  });
});
