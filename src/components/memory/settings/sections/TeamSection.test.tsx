// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { i18n } from "../../../../i18n";
import TeamSection from "./TeamSection";
import { listSpaces, teamAddMember, teamStatus, type TeamStatus } from "../../../../lib/tauri";

vi.mock("../../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../../lib/tauri")>("../../../../lib/tauri");
  return {
    ...actual,
    teamStatus: vi.fn(),
    teamAddMember: vi.fn(),
    teamUpdateMember: vi.fn(),
    teamRemoveMember: vi.fn(),
    teamNewMemberKey: vi.fn(),
    teamHostSpace: vi.fn(),
    listSpaces: vi.fn(),
  };
});

const member = (id: string, name: string, spaces: TeamStatus["members"][number]["spaces"], removed_at?: string) => ({
  id, name, spaces, added_at: "2026-11-01T00:00:00Z", removed_at: removed_at ?? null,
});

const on: TeamStatus = {
  listening: "100.70.1.2:7879",
  spaces: ["Product"],
  members: [
    member("m1", "Amy", [{ space: "Product", access: "write" }]),
    member("m2", "Bo", [{ space: "Product", access: "read" }]),
    member("m3", "Gone", [{ space: "Product", access: "write" }], "2026-11-05T00:00:00Z"),
  ],
};

function renderSection() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}><TeamSection /></QueryClientProvider>);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  localStorage.clear();
  HTMLDialogElement.prototype.showModal ??= function showModal(this: HTMLDialogElement) { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close ??= function close(this: HTMLDialogElement) { this.removeAttribute("open"); };
  vi.mocked(listSpaces).mockResolvedValue([]);
});
afterEach(() => vi.clearAllMocks());

describe("TeamSection", () => {
  it("says this computer is not a team server and shows nothing else", async () => {
    vi.mocked(teamStatus).mockResolvedValue({ members: [], spaces: [], listening: null });
    renderSection();
    expect(await screen.findByText(/not a team server yet/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "How to set one up" }));
    expect(openExternal).toHaveBeenCalledWith("https://github.com/wenlan-app/wenlan/blob/main/docs/team-server.md");
    expect(screen.queryByText("People")).not.toBeInTheDocument();
  });

  it("lists people who are still on the team, never removed ones or ids", async () => {
    vi.mocked(teamStatus).mockResolvedValue(on);
    renderSection();
    expect(await screen.findByText("Team server · on · 100.70.1.2:7879")).toBeInTheDocument();
    expect(screen.getByText("Amy")).toBeInTheDocument();
    expect(screen.getByText("Bo")).toBeInTheDocument();
    expect(screen.getByText("Product · Read only")).toBeInTheDocument();
    expect(screen.queryByText("Gone")).not.toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/\bm1\b|\bm2\b/);
  });

  it("shows the join code once after adding a person", async () => {
    vi.mocked(teamStatus).mockResolvedValue(on);
    vi.mocked(teamAddMember).mockResolvedValue({
      member: member("m4", "Cy", [{ space: "Product", access: "write" }]),
      key: "wlt_secret",
      join_code: "http://100.70.1.2:7879#wlt_secret",
    });
    renderSection();
    await userEvent.click(await screen.findByRole("button", { name: "+ Add person" }));
    const dialog = await screen.findByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "Cy");
    expect(within(dialog).getByLabelText("Address members use to reach this server")).toHaveValue("http://100.70.1.2:7879");
    await userEvent.click(within(dialog).getByRole("button", { name: "Add" }));
    await waitFor(() => expect(teamAddMember).toHaveBeenCalledWith("Cy", [{ space: "Product", access: "write" }], "http://100.70.1.2:7879"));
    const code = await screen.findByLabelText("Join code");
    expect(code).toHaveValue("http://100.70.1.2:7879#wlt_secret");
    expect(code).toHaveAttribute("readonly");
    expect(screen.getByRole("dialog")).toHaveTextContent("Join code for Cy (shown only this once; send it privately)");
    expect(localStorage.getItem("wenlan-team-url")).toBe("http://100.70.1.2:7879");
    await userEvent.click(screen.getByRole("button", { name: "Done" }));
    expect(screen.queryByLabelText("Join code")).not.toBeInTheDocument();
  });
});
