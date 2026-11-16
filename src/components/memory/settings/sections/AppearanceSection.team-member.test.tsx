// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  window.matchMedia ??= ((query: string) => ({
    matches: false, media: query, onchange: null,
    addEventListener: () => {}, removeEventListener: () => {}, addListener: () => {}, removeListener: () => {}, dispatchEvent: () => false,
  })) as unknown as typeof window.matchMedia;
});

vi.mock("../../../../lib/tauri", async (original) => ({
  ...(await original<typeof import("../../../../lib/tauri")>()),
  listThemes: vi.fn(),
}));
vi.mock("./HomeLookSetting", () => ({ default: () => <div data-testid="home-look" /> }));

import { listThemes } from "../../../../lib/tauri";
import { i18n } from "../../../../i18n";
import { resetTeamMeForTests } from "../../../../web/team";
import AppearanceSection from "./AppearanceSection";

function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><AppearanceSection /></QueryClientProvider>);
}

describe("Appearance for a team member", () => {
  beforeEach(async () => {
    await i18n.changeLanguage("en");
    vi.mocked(listThemes).mockReset().mockResolvedValue({ themes_dir: "", themes: [], allowed_fonts: [] });
  });
  afterEach(() => {
    vi.unstubAllGlobals();
    window.history.pushState({}, "", "/");
    resetTeamMeForTests();
  });

  it("the owner gets custom themes and the home look", async () => {
    renderSection();
    expect(await screen.findByText(i18n.t("appearance.newTheme"))).toBeInTheDocument();
    expect(screen.getByTestId("home-look")).toBeInTheDocument();
    await waitFor(() => expect(listThemes).toHaveBeenCalled());
  });

  it("a member keeps the built-in themes and never asks for the owner's theme files", async () => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    window.history.pushState({}, "", "/app");
    resetTeamMeForTests();
    renderSection();
    expect(await screen.findByText(i18n.t("appearance.themes"))).toBeInTheDocument();
    expect(screen.getAllByText(i18n.t("appearance.builtIn")).length).toBeGreaterThan(0);
    expect(screen.queryByText(i18n.t("appearance.newTheme"))).toBeNull();
    expect(screen.queryByText(i18n.t("appearance.askAi"))).toBeNull();
    expect(screen.queryByText(i18n.t("appearance.loadFailed"))).toBeNull();
    expect(screen.queryByTestId("home-look")).toBeNull();
    expect(listThemes).not.toHaveBeenCalled();
  });
});
