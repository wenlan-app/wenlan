// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import * as tauri from "../../../lib/tauri";
import { LinkedVaultAsk, vaultToAsk } from "./LinkedVaultAsk";

vi.mock("../../../lib/tauri");

const yesterday = Math.floor(Date.now() / 1000) - 26 * 3600;
const justNow = Math.floor(Date.now() / 1000);

function renderAsk() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><LinkedVaultAsk /></QueryClientProvider>);
}

beforeEach(() => {
  vi.resetAllMocks();
});

describe("LinkedVaultAsk", () => {
  it("asks once, from the day after linking, and only about a vault not yet answered", () => {
    const now = new Date();
    expect(vaultToAsk([{ path: "/v", folder: "travel", linked_at: justNow }], now)).toBeNull();
    expect(vaultToAsk([{ path: "/v", folder: "travel", linked_at: yesterday, ai_edits: false }], now)).toBeNull();
    expect(vaultToAsk([{ path: "/v", folder: "travel", linked_at: yesterday, ai_edits: true }], now)).toBeNull();
    expect(vaultToAsk([{ path: "/v", folder: "travel", linked_at: yesterday }], now)?.folder).toBe("travel");
  });

  it("lets the AI edit the vault only when the person says yes", async () => {
    vi.mocked(tauri.listLinkedVaults).mockResolvedValue({ vaults: [{ path: "/Users/me/travel", folder: "travel", linked_at: yesterday }] });
    vi.mocked(tauri.setLinkedVaultAiEdits).mockResolvedValue({ vaults: [{ path: "/Users/me/travel", folder: "travel", linked_at: yesterday, ai_edits: true }] });
    renderAsk();
    expect(await screen.findByText("Let the AI edit the notes in travel directly?")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Yes" }));
    await vi.waitFor(() => expect(screen.queryByText("Let the AI edit the notes in travel directly?")).not.toBeInTheDocument());
    expect(tauri.setLinkedVaultAiEdits).toHaveBeenCalledExactlyOnceWith("/Users/me/travel", true);
  });

  it("keeps a no and does not ask again", async () => {
    vi.mocked(tauri.listLinkedVaults).mockResolvedValue({ vaults: [{ path: "/Users/me/travel", folder: "travel", linked_at: yesterday }] });
    vi.mocked(tauri.setLinkedVaultAiEdits).mockResolvedValue({ vaults: [{ path: "/Users/me/travel", folder: "travel", linked_at: yesterday, ai_edits: false }] });
    renderAsk();
    fireEvent.click(await screen.findByRole("button", { name: "No" }));
    await vi.waitFor(() => expect(screen.queryByRole("group")).not.toBeInTheDocument());
    expect(tauri.setLinkedVaultAiEdits).toHaveBeenCalledExactlyOnceWith("/Users/me/travel", false);
  });

  it("shows nothing when linked vaults cannot be read", async () => {
    vi.mocked(tauri.listLinkedVaults).mockRejectedValue(new Error("Review flavor"));
    renderAsk();
    await vi.waitFor(() => expect(tauri.listLinkedVaults).toHaveBeenCalled());
    expect(screen.queryByRole("group")).not.toBeInTheDocument();
  });
});
