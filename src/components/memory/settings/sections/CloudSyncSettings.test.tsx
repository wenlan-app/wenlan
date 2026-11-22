// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { i18n } from "../../../../i18n";
import { cloudStatus, connectCloud, disconnectCloud, type CloudStatus } from "../../../../lib/tauri";
import CloudSyncSettings from "./CloudSyncSettings";

vi.mock("../../../../lib/tauri", () => ({
  cloudStatus: vi.fn(),
  connectCloud: vi.fn(),
  disconnectCloud: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn(() => Promise.resolve()) }));

const URL = "https://wenlan.app";
const off: CloudStatus = { state: "off", url: URL };
const start = { user_code: "WLN4-F7KQ", verification_uri: `${URL}/device`, expires_in: 600 };
const waiting: CloudStatus = { state: "waiting", url: URL, start };
const connected: CloudStatus = {
  state: "connected",
  url: URL,
  last_ok_at: new Date(Date.now() - 120_000).toISOString(),
};

function renderRow() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <CloudSyncSettings />
    </QueryClientProvider>,
  );
}

beforeEach(async () => {
  Object.defineProperty(HTMLDialogElement.prototype, "showModal", { configurable: true, value(this: HTMLDialogElement) { this.open = true; } });
  Object.defineProperty(HTMLDialogElement.prototype, "close", { configurable: true, value(this: HTMLDialogElement) { this.open = false; } });
  await i18n.changeLanguage("en");
  vi.mocked(cloudStatus).mockReset();
  vi.mocked(connectCloud).mockReset();
  vi.mocked(disconnectCloud).mockReset();
  vi.mocked(openExternal).mockClear();
});

describe("Wenlan cloud in Settings › Sync", () => {
  it("signs in through the browser and shows the code to allow", async () => {
    vi.mocked(cloudStatus).mockResolvedValueOnce(off).mockResolvedValue(waiting);
    vi.mocked(connectCloud).mockResolvedValue(start);
    renderRow();
    await userEvent.click(await screen.findByRole("button", { name: "Sign in…" }));
    expect(connectCloud).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(openExternal).toHaveBeenCalledWith(start.verification_uri));
    expect(await screen.findByText("In the browser, allow the code WLN4-F7KQ")).toBeInTheDocument();
  });

  it("says when it last synced and disconnects only after asking", async () => {
    vi.mocked(cloudStatus).mockResolvedValue(connected);
    vi.mocked(disconnectCloud).mockResolvedValue(off);
    renderRow();
    expect(await screen.findByText(/^Synced /)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "wenlan.app" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Disconnect…" }));
    expect(disconnectCloud).not.toHaveBeenCalled();
    expect(screen.getByText("Disconnect? Pages stay here, and the cloud keeps a copy.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    await waitFor(() => expect(disconnectCloud).toHaveBeenCalledTimes(1));
    expect(await screen.findByRole("button", { name: "Sign in…" })).toBeInTheDocument();
  });

  it("shows why connecting stopped", async () => {
    vi.mocked(cloudStatus).mockResolvedValue({ ...off, error: "this computer was not allowed" });
    renderRow();
    expect(await screen.findByText("Couldn't connect: this computer was not allowed")).toBeInTheDocument();
  });
});
