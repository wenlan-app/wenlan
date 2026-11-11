// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { i18n } from "../../../../i18n";
import TeamSection from "./TeamSection";
import { teamStatus, type TeamStatus } from "../../../../lib/tauri";

vi.mock("../../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../../lib/tauri")>("../../../../lib/tauri");
  return { ...actual, teamStatus: vi.fn() };
});

const on: TeamStatus = {
  listening: "0.0.0.0:7879",
  spaces: ["Product", "Roadmap"],
  members: [{ id: "m1", name: "Amy", spaces: [{ space: "Product", access: "write" }], added_at: "2026-11-01T00:00:00Z" }],
};

function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><TeamSection /></QueryClientProvider>);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  localStorage.clear();
});
afterEach(() => vi.clearAllMocks());

describe("TeamSection", () => {
  it("says this computer is not a team server and shows nothing else", async () => {
    vi.mocked(teamStatus).mockResolvedValue({ members: [], spaces: [], listening: null });
    renderSection();
    expect(await screen.findByText(/not a team server yet/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "How to set one up" }));
    expect(openExternal).toHaveBeenCalledWith("https://github.com/wenlan-app/wenlan/blob/main/docs/team-server.md");
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("shows the server, the shared Spaces and where to add people, but no member list", async () => {
    vi.mocked(teamStatus).mockResolvedValue(on);
    renderSection();
    expect(await screen.findByText("Team server · on · 0.0.0.0:7879")).toBeInTheDocument();
    expect(screen.getByText("Product")).toBeInTheDocument();
    expect(screen.getByText("Roadmap")).toBeInTheDocument();
    expect(screen.getByText("Add people in each Space's members window.")).toBeInTheDocument();
    expect(screen.queryByText("Amy")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Add person/ })).not.toBeInTheDocument();
  });

  it("remembers the address people reach the server at", async () => {
    vi.mocked(teamStatus).mockResolvedValue(on);
    renderSection();
    const field = await screen.findByLabelText("Address members use to reach this server");
    expect(field).toHaveValue("");
    await userEvent.type(field, "https://team.example.com");
    expect(localStorage.getItem("wenlan-team-url")).toBe("https://team.example.com");
  });
});
