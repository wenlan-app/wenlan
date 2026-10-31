// SPDX-License-Identifier: AGPL-3.0-only
//
// Pins the one true privacy claim in the Settings screen. The app used to
// state a categorical "no data ever leaves your machine" in two places
// (the settings sidebar's bottom strip and the per-section footer) while the
// same screen offers an Anthropic API key and an unauthenticated Remote
// Access tunnel — both of which do send data off the device. The fix keeps
// the claim in exactly one place (the per-section footer) and makes it
// conditional. This test renders the sidebar and the page as the siblings
// they are in Main.tsx's settings view, so "exactly once" is checked against
// the real composed screen, not each component in isolation.
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";

import SettingsSidebar from "./settings/SettingsSidebar";
import SettingsPage from "./SettingsPage";
import type { SettingsSection } from "./settings/SettingsSidebar";

const shell = vi.hoisted(() => ({ open: vi.fn() }));

vi.mock("@tauri-apps/api/app", () => ({
  getVersion: vi.fn(() => new Promise(() => {})),
}));

vi.mock("@tauri-apps/plugin-shell", () => ({ open: shell.open }));

vi.mock("../../lib/theme", () => ({
  useTheme: () => ["system", vi.fn()] as const,
}));

vi.mock("../../lib/tauri", () => ({
  deleteAgent: vi.fn(() => Promise.resolve()),
  detectMcpClients: vi.fn(() => Promise.resolve([])),
  getCaptureStats: vi.fn(() => Promise.resolve({ clipboard: 0, screen: 0 })),
  getDaemonVersion: vi.fn(() => Promise.resolve("0.18.16")),
  getKnowledgePath: vi.fn(() => Promise.resolve("/Users/test/Wenlan/pages")),
  getBackgroundAiEnabled: vi.fn(() => Promise.resolve(false)),
  getLockMyPages: vi.fn(() => Promise.resolve(false)),
  getExternalLlm: vi.fn(() => Promise.resolve([null, null])),
  getExternalLlmKeyConfigured: vi.fn(() => Promise.resolve(false)),
  getModelChoice: vi.fn(() => Promise.resolve([null, null])),
  getOnDeviceModel: vi.fn(() => Promise.resolve({ models: [], loaded: null, selected: null })),
  getResolvedRouting: vi.fn(() => Promise.resolve(null)),
  getProfile: vi.fn(() =>
    Promise.resolve({
      id: "p1",
      name: "Lucian",
      display_name: "Lucian",
      email: null,
      bio: null,
      avatar_path: null,
      created_at: 0,
    }),
  ),
  getTelemetryStatus: vi.fn(() =>
    Promise.resolve({ enabled: false, available: true, pending_operations: 0 }),
  ),
  isRunAtLoginEnabled: vi.fn(() => Promise.resolve(false)),
  listAgents: vi.fn(() => Promise.resolve([])),
  setAvatar: vi.fn(() => Promise.resolve()),
  setRunAtLogin: vi.fn(() => Promise.resolve()),
  setTelemetryEnabled: vi.fn(() =>
    Promise.resolve({ enabled: false, available: true, pending_operations: 0 }),
  ),
  setSetupCompleted: vi.fn(() => Promise.resolve()),
  removeAvatar: vi.fn(() => Promise.resolve()),
  updateAgent: vi.fn(() => Promise.resolve()),
  updateProfile: vi.fn(() => Promise.resolve()),
}));

vi.mock("./sources/SourcesSection", () => ({
  default: () => <div />,
}));

vi.mock("../ChatImport/ImportFlow", () => ({
  ImportFlow: () => <div />,
}));

vi.mock("./RemoteAccessPanel", () => ({
  RemoteAccessPanel: () => <div />,
}));

vi.mock("../intelligence/IntelligenceSetup", () => ({
  AnthropicFields: () => <div />,
  OnDeviceModelCard: () => <div />,
  useApiKeyStatus: () => ({
    data: { hasAnthropic: false, hasOpenAI: false },
    isLoading: false,
  }),
}));

vi.mock("./settings/sections/DiagnosticsSection", () => ({
  default: () => <div />,
}));

function renderSettingsScreen(section: SettingsSection = "general") {
  const queryClient = new QueryClient({
    defaultOptions: {
      queries: { retry: false },
      mutations: { retry: false },
    },
  });

  // Mirrors Main.tsx: SettingsSidebar and SettingsPage are mounted as
  // siblings under the same "settings" view — not two isolated trees.
  return render(
    <QueryClientProvider client={queryClient}>
      <div>
        <SettingsSidebar
          hidden={false}
          mode="labels"
          active="general"
          onSelect={() => {}}
        />
        <SettingsPage section={section} onBack={() => {}} onImport={() => {}} />
      </div>
    </QueryClientProvider>,
  );
}

describe("Settings privacy policy entry", () => {
  it.each([
    ["general", "General"],
    ["sources", "Sources"],
    ["agents", "Connections"],
    ["intelligence", "Intelligence"],
    ["diagnostics", "Diagnostics"],
  ] as const)("keeps Privacy out of the %s header and shows one policy-block link", (section, title) => {
    renderSettingsScreen(section);

    const header = screen.getByRole("banner");
    expect(within(header).getByRole("heading", { name: title })).toBeInTheDocument();
    expect(within(header).queryByText("Privacy")).not.toBeInTheDocument();
    expect(within(header).queryByRole("button", { name: "Details: Privacy" })).not.toBeInTheDocument();
    const links = screen.getAllByRole("link", { name: "Privacy policy" });
    expect(links).toHaveLength(1);
    expect(links[0]).toHaveAttribute(
      "href",
      "https://github.com/wenlan-app/wenlan/blob/main/docs/PRIVACY.md",
    );
    expect(links[0].closest(".settings-sidebar-policy")).not.toBeNull();
  });

  it("opens the policy in the external browser when activated", async () => {
    shell.open.mockResolvedValue(undefined);
    renderSettingsScreen();

    await userEvent.click(screen.getByRole("link", { name: "Privacy policy" }));

    expect(shell.open).toHaveBeenCalledWith("https://github.com/wenlan-app/wenlan/blob/main/docs/PRIVACY.md");
  });

  it("shows a localized error when the external browser cannot open the policy", async () => {
    shell.open.mockRejectedValue(new Error("open failed"));
    renderSettingsScreen();

    await userEvent.click(screen.getByRole("link", { name: "Privacy policy" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn’t open the link. Try again.");
  });
});
