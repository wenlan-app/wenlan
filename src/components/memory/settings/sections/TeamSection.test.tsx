// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { open as openExternal } from "@tauri-apps/plugin-shell";
import { i18n } from "../../../../i18n";
import TeamSection from "./TeamSection";
import { getProfile, listShares, teamFirewall, teamFirewallAllow, teamJoin, teamStart, teamStatus, type TeamJoinResponse, type TeamStatus } from "../../../../lib/tauri";

vi.mock("../../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../../lib/tauri")>("../../../../lib/tauri");
  return { ...actual, teamStatus: vi.fn(), listShares: vi.fn(), teamJoin: vi.fn(), teamFirewall: vi.fn(), teamFirewallAllow: vi.fn(), teamStart: vi.fn(), getProfile: vi.fn() };
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
  // Not on Windows: nothing in the way.
  vi.mocked(teamFirewall).mockResolvedValue(null);
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
    await vi.waitFor(() => expect(copy).toBeEnabled());
    const command = screen.getByText(/docker run/).closest("pre") as HTMLElement;
    // The key this Wenlan made goes in the command by its SHA-256 only:
    // nothing to read back from the server, and no key on its command line.
    await vi.waitFor(() => expect(command.textContent).toMatch(/wenlan-server team "https:\/\/wenlan\.acme\.com" --owner [0-9a-f]{64}$/));
    expect(command.textContent).not.toContain("wlt_");
    expect(command.textContent).not.toContain("#");
    expect(command.textContent).not.toContain("WENLAN_");
    expect(command.textContent).toContain("-p 7879:7879 ");
    expect(command.textContent).not.toContain("7878");
    // One line: nothing has to be read from the server's output.
    expect(command.textContent).not.toContain("docker logs");
    await userEvent.click(screen.getByRole("button", { name: "More about servers" }));
    expect(openExternal).toHaveBeenCalledWith("https://github.com/wenlan-app/wenlan/blob/main/docs/team-server.md");
    await userEvent.click(screen.getByRole("button", { name: "Paste the owner's link instead…" }));
    expect(screen.queryByText("Run this on the server")).not.toBeInTheDocument();
    expect(await screen.findByLabelText("Invite link")).toBeInTheDocument();
  });

  it("joins the new server by itself once it is up, with the owner's link it made", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    try {
      const user = userEvent.setup({ advanceTimers: vi.advanceTimersByTime });
      vi.mocked(teamStatus).mockResolvedValue({ members: [], spaces: [], listening: null });
      const answer = {
        url: "https://wenlan.acme.com",
        me: { member_id: "o1", name: "Qi", role: "owner", spaces: [], team: { id: "t1", name: "Acme" } },
      } as unknown as TeamJoinResponse;
      vi.mocked(teamJoin).mockRejectedValueOnce(new Error("connection refused")).mockResolvedValue(answer);
      renderSection();
      await user.click(await screen.findByRole("button", { name: "Show steps…" }));
      await user.type(screen.getByPlaceholderText("https://wenlan.example.com"), "https://wenlan.acme.com");
      await vi.waitFor(() => expect(screen.getByRole("button", { name: "Copy" })).toBeEnabled());
      const shown = screen.getByText(/docker run/).textContent ?? "";
      await user.click(screen.getByRole("button", { name: "Copy" }));
      expect(await screen.findByText("Waiting for the server…")).toBeInTheDocument();
      expect(teamJoin).not.toHaveBeenCalled();
      // Not up yet: asked again a little later.
      await vi.advanceTimersByTimeAsync(5000);
      expect(teamJoin).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(5000);
      const link = vi.mocked(teamJoin).mock.calls[1][0];
      expect(link).toMatch(/^https:\/\/wenlan\.acme\.com\/join#wlt_[0-9a-f]{64}$/);
      // The command held the SHA-256 of exactly the key this window joins with.
      const { keySha256 } = await import("../../team/ServerSetupDialog");
      expect(shown).toContain(`--owner ${await keySha256(link.split("#")[1])}`);
      expect(await screen.findByRole("button", { name: "Join Acme" })).toBeInTheDocument();
      expect(screen.queryByText("Run this on the server")).not.toBeInTheDocument();
      // The same key is offered again if the window is opened again before
      // joining: the server already has it.
      expect(localStorage.getItem("wenlan-team-owner-key")).toBe(link.split("#")[1]);
    } finally {
      vi.useRealTimers();
    }
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

  const blocked = { program: "C:\\Users\\amy\\AppData\\Local\\Wenlan\\wenlan-server.exe", open: false, public_network: true };

  it("says Windows Firewall keeps teammates out, and lets them in when Windows allows it", async () => {
    vi.mocked(teamStatus).mockResolvedValue(on);
    vi.mocked(teamFirewall).mockResolvedValueOnce(blocked).mockResolvedValue({ ...blocked, open: true });
    vi.mocked(teamFirewallAllow).mockResolvedValue();
    renderSection();
    expect(await screen.findByText("Other computers can't connect yet")).toBeInTheDocument();
    expect(screen.getByText(/Windows Firewall is blocking the team on this computer\. .*public networks/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Allow" }));
    expect(teamFirewallAllow).toHaveBeenCalledWith(blocked);
    await vi.waitFor(() => expect(screen.queryByText("Other computers can't connect yet")).not.toBeInTheDocument());
  });

  it("asks again when Windows was told no", async () => {
    vi.mocked(teamStatus).mockResolvedValue(on);
    vi.mocked(teamFirewall).mockResolvedValue({ ...blocked, public_network: false });
    vi.mocked(teamFirewallAllow).mockRejectedValue(new Error("declined"));
    renderSection();
    await userEvent.click(await screen.findByRole("button", { name: "Allow" }));
    expect(await screen.findByText("Windows didn't allow it. Press Allow again and choose Yes.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Allow" })).toBeEnabled();
  });

  it("does not ask Windows while the team is off", async () => {
    vi.mocked(teamStatus).mockResolvedValue({ ...on, listening: null });
    renderSection();
    expect(await screen.findByText("Off")).toBeInTheDocument();
    expect(teamFirewall).not.toHaveBeenCalled();
  });

  it("starts the team without a prompt of its own: Windows asks itself, and Settings follows up", async () => {
    vi.mocked(teamStatus).mockResolvedValue({ members: [], spaces: [], listening: null, suggested_urls: ["http://192.168.1.20:7879"] });
    vi.mocked(getProfile).mockResolvedValue({ display_name: "Amy" } as Awaited<ReturnType<typeof getProfile>>);
    vi.mocked(teamStart).mockResolvedValue({ members: [], spaces: [], listening: "0.0.0.0:7879" });
    renderSection();
    await userEvent.click(await screen.findByRole("button", { name: "Start…" }));
    await userEvent.type(screen.getByLabelText(/^Team name/), "Acme");
    await vi.waitFor(() => expect(screen.getByLabelText(/^Your name/)).toHaveValue("Amy"));
    await userEvent.click(screen.getByRole("button", { name: "Start" }));
    await vi.waitFor(() => expect(teamStart).toHaveBeenCalledWith("Acme", "Amy", "http://192.168.1.20:7879"));
    expect(teamFirewallAllow).not.toHaveBeenCalled();
  });
});
