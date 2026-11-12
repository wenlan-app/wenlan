// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { i18n } from "../../../../i18n";
import TeamSection from "./TeamSection";
import { listShares, teamStatus, type TeamStatus } from "../../../../lib/tauri";

vi.mock("../../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../../lib/tauri")>("../../../../lib/tauri");
  return { ...actual, teamStatus: vi.fn(), listShares: vi.fn() };
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
  HTMLDialogElement.prototype.showModal ??= function showModal(this: HTMLDialogElement) { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close ??= function close(this: HTMLDialogElement) { this.removeAttribute("open"); };
  await i18n.changeLanguage("en");
  vi.mocked(listShares).mockResolvedValue([]);
  localStorage.clear();
});
afterEach(() => vi.clearAllMocks());

describe("TeamSection", () => {
  it("offers the three ways into a team, one row each, when this computer is in none", async () => {
    vi.mocked(teamStatus).mockResolvedValue({ members: [], spaces: [], listening: null });
    renderSection();
    expect(await screen.findByText("Join a team")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Paste invite link…" })).toBeInTheDocument();
    expect(screen.getByText("Start a team on this computer")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start…" })).toBeInTheDocument();
    expect(screen.getByText("Run it on a server")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show steps…" })).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("walks through a server in the app: address, the command for it, then the owner's link", async () => {
    vi.mocked(teamStatus).mockResolvedValue({ members: [], spaces: [], listening: null });
    renderSection();
    await userEvent.click(await screen.findByRole("button", { name: "Show steps…" }));
    const copy = screen.getByRole("button", { name: "Copy" });
    // Nothing to copy until the address is one a shell can take as is.
    expect(copy).toBeDisabled();
    await userEvent.type(screen.getByPlaceholderText("https://wenlan.example.com"), "https://wenlan.acme.com/");
    expect(copy).toBeEnabled();
    const command = screen.getByText(/docker run/).closest("pre") as HTMLElement;
    expect(command.textContent).toContain("wenlan-server team https://wenlan.acme.com");
    expect(command.textContent).not.toContain("WENLAN_");
    expect(command.textContent).toContain("-p 7879:7879 ");
    expect(command.textContent).not.toContain("7878");
    expect(command.textContent).toContain("docker logs wenlan");
    await userEvent.click(screen.getByRole("button", { name: "More about servers" }));
    expect(openExternal).toHaveBeenCalledWith("https://github.com/wenlan-app/wenlan/blob/main/docs/team-server.md");
    await userEvent.click(screen.getByRole("button", { name: "Paste the owner's link…" }));
    expect(screen.queryByText("Run this on the server")).not.toBeInTheDocument();
    expect(await screen.findByLabelText("Invite link")).toBeInTheDocument();
  });

  it("refuses an address a shell would read differently", async () => {
    const { usableServerUrl } = await import("../../team/ServerSetupDialog");
    expect(usableServerUrl("http://100.64.0.3:7879")).toBe("http://100.64.0.3:7879");
    expect(usableServerUrl("https://a.example.com/")).toBe("https://a.example.com");
    expect(usableServerUrl("https://a.example.com; rm -rf /")).toBeNull();
    expect(usableServerUrl("wenlan.example.com")).toBeNull();
    expect(usableServerUrl("https://$(whoami).example.com")).toBeNull();
  });

  it("on the team's server: its name, that it is on, and no Spaces or people", async () => {
    vi.mocked(teamStatus).mockResolvedValue({ ...on, team: { id: "t1", name: "Acme" } });
    renderSection();
    expect(await screen.findByText("Acme")).toBeInTheDocument();
    expect(screen.getByText("This computer is the team's server")).toBeInTheDocument();
    expect(screen.getByText("On · 0.0.0.0:7879")).toBeInTheDocument();
    expect(screen.queryByText("Product")).not.toBeInTheDocument();
    expect(screen.queryByText("Amy")).not.toBeInTheDocument();
  });

  it("on a member's computer: the team and who they joined as", async () => {
    vi.mocked(teamStatus).mockRejectedValue(new Error("not a server"));
    vi.mocked(listShares).mockResolvedValue([{
      kind: "space", target: "Product", folder: "", host: "https://team.example.com", team: { id: "t1", name: "Acme" },
      member_id: "me", member_name: "Dee", members: [], last: null,
    }]);
    renderSection();
    expect(await screen.findByText("Acme")).toBeInTheDocument();
    expect(screen.getByText("Joined as Dee")).toBeInTheDocument();
    expect(screen.queryByText(/team\.example\.com/)).not.toBeInTheDocument();
  });

  it("remembers the address put in invite links", async () => {
    vi.mocked(teamStatus).mockResolvedValue(on);
    renderSection();
    const field = await screen.findByLabelText("Address in invite links");
    expect(field).toHaveValue("");
    await userEvent.type(field, "https://team.example.com");
    expect(localStorage.getItem("wenlan-team-url")).toBe("https://team.example.com");
  });
});
