// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import "../../../i18n";

const api = vi.hoisted(() => ({ enabled: vi.fn(), set: vi.fn() }));
vi.mock("../../../lib/tauri", () => ({
  skillsFromLibraryEnabled: api.enabled,
  setSkillsFromLibraryEnabled: api.set,
}));

import { HowIWorkSetting } from "./HowIWorkSetting";

function renderSetting() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={queryClient}>
      <HowIWorkSetting />
    </QueryClientProvider>,
  );
}

describe("How-I-work setting", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    let value = true;
    api.enabled.mockImplementation(async () => value);
    api.set.mockImplementation(async (on: boolean) => { value = on; });
  });

  it("UX-SKILL-1 one Settings switch turns it off and back on", async () => {
    renderSetting();
    const toggle = await screen.findByRole("button", { name: "Let AI tools follow how you work" });
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(toggle);
    await waitFor(() => expect(screen.getByRole("button", { name: "Let AI tools follow how you work" })).toHaveAttribute("aria-pressed", "false"));
    expect(api.set).toHaveBeenLastCalledWith(false);
    fireEvent.click(screen.getByRole("button", { name: "Let AI tools follow how you work" }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Let AI tools follow how you work" })).toHaveAttribute("aria-pressed", "true"));
    expect(api.set).toHaveBeenLastCalledWith(true);
  });

  it("UX-SKILL-1 keeps the old position and says so when saving fails", async () => {
    api.set.mockRejectedValue(new Error("no"));
    renderSetting();
    fireEvent.click(await screen.findByRole("button", { name: "Let AI tools follow how you work" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn’t change this. Try again.");
    expect(screen.getByRole("button", { name: "Let AI tools follow how you work" })).toHaveAttribute("aria-pressed", "true");
  });
});
