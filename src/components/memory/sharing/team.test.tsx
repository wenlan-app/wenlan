// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { JoinSharedDialog } from "./JoinSharedDialog";
import { ShareDialog } from "./ShareDialog";
import { groupTeamSpaces } from "./sharing";
import { joinTeamSpace, listShares, listSpaces, teamJoin, type SharedStatus } from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../lib/tauri")>("../../../lib/tauri");
  return {
    ...actual,
    listShares: vi.fn(),
    listSpaces: vi.fn(),
    teamJoin: vi.fn(),
    joinTeamSpace: vi.fn(),
    createSpace: vi.fn(),
    lookInSharedFolder: vi.fn(),
  };
});

const teamShare: SharedStatus = {
  kind: "space",
  target: "Product",
  folder: "",
  host: "https://team.example.com/",
  read_only: true,
  member_id: "me",
  member_name: "Dee",
  members: [{ id: "me", name: "Dee" }, { id: "a", name: "Amy" }],
  last: { at: "2026-11-11T00:00:00Z", counts: { exported: 0, applied: 0, conflicts: 0, skipped: 0 }, paused: "the team server no longer accepts this member key" },
};

function wrap(node: React.ReactNode) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(<QueryClientProvider client={client}>{node}</QueryClientProvider>);
}

beforeEach(async () => {
  await i18n.changeLanguage("en");
  HTMLDialogElement.prototype.showModal ??= function showModal(this: HTMLDialogElement) { this.setAttribute("open", ""); };
  HTMLDialogElement.prototype.close ??= function close(this: HTMLDialogElement) { this.removeAttribute("open"); };
  vi.mocked(listShares).mockResolvedValue([]);
  vi.mocked(listSpaces).mockResolvedValue([]);
});
afterEach(() => vi.clearAllMocks());

describe("JoinSharedDialog code mode", () => {
  it("lists the server's Spaces and joins the chosen one", async () => {
    vi.mocked(teamJoin).mockResolvedValue({
      url: "https://team.example.com",
      me: { member_id: "me", name: "Dee", spaces: [{ space: "Product", access: "write" }, { space: "Research", access: "read" }] },
    });
    vi.mocked(joinTeamSpace).mockResolvedValue({ exported: 0, applied: 3, conflicts: 0, skipped: 0 });
    const onJoined = vi.fn();
    wrap(<JoinSharedDialog onClose={() => undefined} onJoined={onJoined} />);
    await userEvent.click(screen.getByRole("radio", { name: "A team server join code" }));
    const code = "https://team.example.com#wlt_abc";
    await userEvent.click(screen.getByLabelText("Join code"));
    await userEvent.paste(code);
    await screen.findByText("Available to join:");
    expect(teamJoin).toHaveBeenCalledWith(code);
    expect(screen.getByRole("radio", { name: "Research · Read only" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("radio", { name: "Research · Read only" }));
    await userEvent.click(screen.getByRole("button", { name: "Add" }));
    await waitFor(() => expect(joinTeamSpace).toHaveBeenCalledWith("Research", code, "Research"));
    expect(onJoined).toHaveBeenCalledWith("space", "Research");
  });

  it("asks for another local name when one is taken", async () => {
    vi.mocked(listSpaces).mockResolvedValue([{ id: "1", name: "Product" } as never]);
    vi.mocked(teamJoin).mockResolvedValue({ url: "u", me: { member_id: "me", name: "Dee", spaces: [{ space: "Product", access: "write" }] } });
    wrap(<JoinSharedDialog onClose={() => undefined} />);
    await userEvent.click(screen.getByRole("radio", { name: "A team server join code" }));
    await userEvent.click(screen.getByLabelText("Join code"));
    await userEvent.paste("u#wlt_x");
    expect(await screen.findByText("You already have a Space with this name. Name this one:")).toBeInTheDocument();
  });
});

describe("ShareDialog team Space", () => {
  it("shows the server instead of the folder and says when the key stopped working", async () => {
    vi.mocked(listShares).mockResolvedValue([teamShare]);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByText("Team server · team.example.com")).toBeInTheDocument();
    expect(screen.getByText("Dee (you) · Read only")).toBeInTheDocument();
    expect(screen.getByText("Paused · the server no longer accepts your join code")).toBeInTheDocument();
    expect(screen.queryByText(/Show in|Open folder/)).not.toBeInTheDocument();
    expect(screen.queryByText("Folder")).not.toBeInTheDocument();
  });

  it("links a hosted Space to Settings › Team", async () => {
    vi.mocked(listShares).mockResolvedValue([{ ...teamShare, host: null, read_only: false, hosted: true, last: null }]);
    const onOpenSettings = vi.fn();
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} onOpenSettings={onOpenSettings} />);
    await userEvent.click(await screen.findByRole("button", { name: "Manage people in Settings › Team" }));
    expect(onOpenSettings).toHaveBeenCalledWith("team");
  });
});

describe("groupTeamSpaces", () => {
  it("keeps own Spaces apart and groups by server", () => {
    const spaces = [{ name: "Mine" }, { name: "Product" }, { name: "Hosted" }];
    const shares = [teamShare, { ...teamShare, target: "Hosted", host: null, hosted: true }];
    const { own, groups } = groupTeamSpaces(spaces, shares);
    expect(own).toEqual([{ name: "Mine" }]);
    expect(groups).toEqual([{ host: "team.example.com", spaces: [{ name: "Product" }] }, { host: null, spaces: [{ name: "Hosted" }] }]);
  });
});
