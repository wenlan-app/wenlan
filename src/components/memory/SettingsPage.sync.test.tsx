// SPDX-License-Identifier: AGPL-3.0-only
import { beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../i18n";
import SettingsPage from "./SettingsPage";
import SettingsSidebar, { SETTINGS_GROUPS } from "./settings/SettingsSidebar";

vi.mock("@tauri-apps/api/app", () => ({ getVersion: vi.fn(() => new Promise(() => {})) }));
vi.mock("@tauri-apps/plugin-shell", () => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("../../lib/theme", () => ({
  useTheme: () => ["system", vi.fn()] as const,
}));
vi.mock("../../lib/tauri", () => ({
  listShares: vi.fn(() => Promise.resolve([])),
  lookInSharedFolder: vi.fn(),
  revealSharedFolder: vi.fn(() => Promise.resolve()),
  shareLibrary: vi.fn(),
  stopSharingLibrary: vi.fn(),
  setLibraryMainDevice: vi.fn(),
}));

function renderPage() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={client}>
      <SettingsPage section="sync" onBack={() => {}} onImport={() => {}} />
    </QueryClientProvider>,
  );
}

describe("Settings > Sync tab", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("shows the title, the intro and the sync row", async () => {
    renderPage();
    expect(within(screen.getByRole("banner")).getByRole("heading", { name: "Sync" })).toBeInTheDocument();
    expect(screen.getByText("Use the same Wenlan on your other computers and in your phone's AI.")).toBeInTheDocument();
    expect(await screen.findByText("Sync to my other computers")).toBeInTheDocument();
  });

  it("sits right after Connections in the sidebar", () => {
    const ids = SETTINGS_GROUPS.map((g) => g.id);
    expect(ids[ids.indexOf("agents") + 1]).toBe("sync");
    render(<SettingsSidebar hidden={false} mode="labels" active="sync" onSelect={() => {}} />);
    expect(screen.getByRole("button", { name: "Sync" })).toHaveAttribute("data-active", "true");
  });
});
