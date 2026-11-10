// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as tauri from "../../../lib/tauri";
import LinkVaultDialog from "./LinkVaultDialog";

vi.mock("../../../lib/tauri");

function renderDialog(onClose = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><LinkVaultDialog onClose={onClose} /></QueryClientProvider>);
  return { onClose };
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(tauri.detectObsidianVaults).mockResolvedValue([
    { name: "second-brain", path: "/Users/me/second-brain" },
    { name: "work-notes", path: "/Users/me/work-notes" },
  ]);
  vi.mocked(tauri.linkObsidianVault).mockResolvedValue({
    vaults: [{ path: "/Users/me/second-brain", folder: "second-brain" }],
    notes_added: 3,
  });
});

describe("LinkVaultDialog", () => {
  it("links the picked vault into the Wiki and says the AI won't change it", async () => {
    const { onClose } = renderDialog();
    expect(screen.getByText("The AI won't change your vault.")).toBeInTheDocument();
    const link = screen.getByRole("button", { name: "Link" });
    expect(link).toBeDisabled();
    fireEvent.click(await screen.findByRole("button", { name: "second-brain" }));
    expect(screen.getByRole("button", { name: "second-brain" })).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(link);
    await vi.waitFor(() => expect(onClose).toHaveBeenCalled());
    expect(tauri.linkObsidianVault).toHaveBeenCalledExactlyOnceWith("/Users/me/second-brain");
    expect(tauri.addSource).not.toHaveBeenCalled();
  });

  it("says in words why a vault already read as a source cannot be linked", async () => {
    vi.mocked(tauri.linkObsidianVault).mockRejectedValue(new Error("Validation: linked_vault_is_a_source"));
    const { onClose } = renderDialog();
    fireEvent.click(await screen.findByRole("button", { name: "work-notes" }));
    fireEvent.click(screen.getByRole("button", { name: "Link" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This vault is already connected as a source.");
    expect(onClose).not.toHaveBeenCalled();
  });
});
