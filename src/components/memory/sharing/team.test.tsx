// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { JoinSharedDialog } from "./JoinSharedDialog";
import { ShareDialog } from "./ShareDialog";
import { groupTeamSpaces } from "./sharing";
import { teamGroups } from "../team/TeamPage";
import { StartTeamDialog } from "../team/StartTeamDialog";
import {
  joinTeamSpace,
  listShares,
  listSpaces,
  teamAddMember,
  teamHostSpace,
  teamJoin,
  teamRememberMembership,
  teamSetEveryone,
  teamStart,
  getProfile,
  teamStatus,
  teamUpdateMember,
  type SharedStatus,
  type TeamMember,
  type TeamStatus,
} from "../../../lib/tauri";

vi.mock("../../../lib/tauri", async () => {
  const actual = await vi.importActual<typeof import("../../../lib/tauri")>("../../../lib/tauri");
  return {
    ...actual,
    listShares: vi.fn(),
    listSpaces: vi.fn(),
    teamJoin: vi.fn(),
    teamRememberMembership: vi.fn(),
    teamStart: vi.fn(),
    getProfile: vi.fn(),
    teamStatus: vi.fn(),
    teamAddMember: vi.fn(),
    teamUpdateMember: vi.fn(),
    teamNewMemberKey: vi.fn(),
    teamRemoveMember: vi.fn(),
    teamHostSpace: vi.fn(),
    teamSetEveryone: vi.fn(),
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

const person = (id: string, name: string, spaces: TeamMember["spaces"], removed_at?: string): TeamMember =>
  ({ id, name, spaces, added_at: "2026-11-01T00:00:00Z", removed_at: removed_at ?? null });

const hostedShare: SharedStatus = {
  kind: "space", target: "Product", folder: "/x", host: null, hosted: true, read_only: false,
  member_id: "me", member_name: "Dee", members: [{ id: "me", name: "Dee" }],
  last: { at: "2026-11-11T00:00:00Z", counts: { exported: 0, applied: 0, conflicts: 0, skipped: 0 } },
};

const hostedStatus: TeamStatus = {
  listening: "100.70.1.2:7879",
  spaces: ["Product", "Roadmap"],
  members: [
    person("a", "Amy", [{ space: "Product", access: "write" }, { space: "Roadmap", access: "write" }]),
    person("b", "Bo", [{ space: "Product", access: "read" }]),
    person("g", "Gone", [{ space: "Product", access: "write" }], "2026-11-05T00:00:00Z"),
    person("o", "Other", [{ space: "Roadmap", access: "write" }]),
  ],
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
  vi.mocked(teamStatus).mockResolvedValue({ members: [], spaces: [], listening: null });
  localStorage.clear();
});
afterEach(() => vi.clearAllMocks());

describe("JoinSharedDialog code mode", () => {
  it("reads an invite link, then joins the whole team with one confirm", async () => {
    vi.mocked(teamJoin).mockResolvedValue({
      url: "https://team.example.com",
      me: { member_id: "me", name: "Dee", team: { id: "t1", name: "Acme" }, spaces: [{ space: "Product", access: "write" }, { space: "Research", access: "read" }] },
    });
    vi.mocked(joinTeamSpace).mockResolvedValue({ exported: 0, applied: 3, conflicts: 0, skipped: 0 });
    const onJoined = vi.fn();
    wrap(<JoinSharedDialog onClose={() => undefined} onJoined={onJoined} />);
    const link = "https://team.example.com/join#wlt_abc";
    await userEvent.click(screen.getByLabelText("Invite link"));
    await userEvent.paste(link);
    expect(await screen.findByRole("heading", { name: "Join Acme" })).toBeInTheDocument();
    expect(teamJoin).toHaveBeenCalledWith(link);
    expect(screen.getByText("You're joining as Dee. You'll get:")).toBeInTheDocument();
    expect(screen.getByText("Research · Read only")).toBeInTheDocument();
    expect(screen.getByText("Your private Spaces stay yours. The team sees only what is in its Spaces.")).toBeInTheDocument();
    expect(screen.queryByText(/team\.example\.com/)).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Join Acme" }));
    await waitFor(() => expect(joinTeamSpace).toHaveBeenCalledWith("Research", link, "Research"));
    expect(joinTeamSpace).toHaveBeenCalledWith("Product", link, "Product");
    expect(onJoined).toHaveBeenCalledWith("team", "t1");
    // The team is kept even if nothing were open yet.
    expect(teamRememberMembership).toHaveBeenCalledWith(link);
  });

  it("puts a team Space beside a personal one of the same name, never into it", async () => {
    vi.mocked(listSpaces).mockResolvedValue([{ id: "1", name: "Product" } as never]);
    vi.mocked(teamJoin).mockResolvedValue({ url: "https://team.example.com", me: { member_id: "me", name: "Dee", team: { id: "t1", name: "Acme" }, spaces: [{ space: "Product", access: "write" }] } });
    vi.mocked(joinTeamSpace).mockResolvedValue({ exported: 0, applied: 0, conflicts: 0, skipped: 0 });
    wrap(<JoinSharedDialog onClose={() => undefined} />);
    await userEvent.click(screen.getByLabelText("Invite link"));
    await userEvent.paste("https://team.example.com#wlt_x");
    expect(await screen.findByText("Product (Acme)")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Join Acme" }));
    await waitFor(() => expect(joinTeamSpace).toHaveBeenCalledWith("Product (Acme)", "https://team.example.com#wlt_x", "Product"));
  });
});

describe("JoinSharedDialog folder link", () => {
  it("switches to the folder flow and back", async () => {
    wrap(<JoinSharedDialog onClose={() => undefined} />);
    expect(screen.getByLabelText("Invite link")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    expect(screen.getByText("No folder chosen")).toBeInTheDocument();
    expect(screen.queryByLabelText("Invite link")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "‹ Use an invite link instead" }));
    expect(screen.getByLabelText("Invite link")).toBeInTheDocument();
  });
});

describe("ShareDialog team Space", () => {
  it("names the team and says when the key stopped working; a member cannot stop one Space", async () => {
    vi.mocked(listShares).mockResolvedValue([{ ...teamShare, team: { id: "t1", name: "Acme" } }]);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByRole("heading", { name: "Product · Acme" })).toBeInTheDocument();
    expect(screen.queryByText(/team\.example\.com/)).not.toBeInTheDocument();
    expect(screen.getByText("Dee (you) · Read only")).toBeInTheDocument();
    expect(screen.getByText("Amy")).toBeInTheDocument();
    expect(screen.getByText("Paused · the server no longer accepts your join code")).toBeInTheDocument();
    expect(screen.queryByText(/Show in|Open folder/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Stop sharing|Take off/ })).not.toBeInTheDocument();
  });

  it("says the server cannot be reached when it is only off, not that the key stopped", async () => {
    const off = { ...teamShare.last!, paused: "the team server at http://100.80.1.2:7879 cannot be reached" };
    vi.mocked(listShares).mockResolvedValue([{ ...teamShare, team: { id: "t1", name: "Acme" }, last: off }]);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByText("Paused · can't reach the team's server right now")).toBeInTheDocument();
    expect(screen.queryByText(/no longer accepts/)).not.toBeInTheDocument();
  });

  it("lists the people added to a Space that is only for some people", async () => {
    vi.mocked(listShares).mockResolvedValue([hostedShare]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByRole("radio", { name: "Only some people" })).toBeChecked();
    expect(await screen.findByText("Amy")).toBeInTheDocument();
    expect(screen.getByLabelText("Bo's access")).toHaveValue("read");
    expect(screen.getByLabelText("Amy's access")).toHaveValue("write");
    expect(screen.queryByText("Gone")).not.toBeInTheDocument();
    // People come in through the team page's invite, not from here.
    expect(screen.queryByLabelText("Name")).not.toBeInTheDocument();
  });

  it("changes someone's access, keeping their other Spaces", async () => {
    vi.mocked(listShares).mockResolvedValue([hostedShare]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    vi.mocked(teamUpdateMember).mockResolvedValue(person("a", "Amy", []));
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.selectOptions(await screen.findByLabelText("Amy's access"), "read");
    await waitFor(() => expect(teamUpdateMember).toHaveBeenCalledWith("a", {
      spaces: [{ space: "Product", access: "read" }, { space: "Roadmap", access: "write" }],
    }));
  });

  it("adds someone already on the team to the Space", async () => {
    vi.mocked(listShares).mockResolvedValue([hostedShare]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    vi.mocked(teamUpdateMember).mockResolvedValue(person("o", "Other", []));
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.selectOptions(await screen.findByLabelText("+ Add someone from Team"), "o");
    await waitFor(() => expect(teamUpdateMember).toHaveBeenCalledWith("o", {
      spaces: [{ space: "Roadmap", access: "write" }, { space: "Product", access: "write" }],
    }));
    expect(teamAddMember).not.toHaveBeenCalled();
  });

  it("opens the Space to everyone on the team", async () => {
    vi.mocked(listShares).mockResolvedValue([hostedShare]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    vi.mocked(teamSetEveryone).mockResolvedValue();
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("radio", { name: "Everyone on the team" }));
    await waitFor(() => expect(teamSetEveryone).toHaveBeenCalledWith("Product", "write"));
  });

  it("lets the owner of a server elsewhere open its Space to everyone from here", async () => {
    const team = { id: "t1", name: "Acme" };
    vi.mocked(listShares).mockResolvedValue([{ ...teamShare, read_only: false, team, last: { ...teamShare.last!, paused: undefined } }]);
    vi.mocked(teamStatus).mockResolvedValue({ ...hostedStatus, team, remote: "https://team.example.com" });
    vi.mocked(teamSetEveryone).mockResolvedValue();
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("radio", { name: "Everyone on the team" }));
    await waitFor(() => expect(teamSetEveryone).toHaveBeenCalledWith("Product", "write"));
    // Taking a Space off the team stays on the server itself.
    expect(screen.queryByRole("button", { name: /take .* off/i })).toBeNull();
  });
});

describe("ShareDialog share mode", () => {
  it("offers only the cloud folder when this computer is not a team server", async () => {
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByText("Folder")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /With Team/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "‹ Back" })).not.toBeInTheDocument();
  });

  it("hosts the Space for the team and then shows its members", async () => {
    vi.mocked(listShares).mockResolvedValueOnce([]).mockResolvedValue([hostedShare]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    vi.mocked(teamHostSpace).mockResolvedValue({ exported: 0, applied: 0, conflicts: 0, skipped: 0 });
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /With Team/ }));
    await waitFor(() => expect(teamHostSpace).toHaveBeenCalledWith("Product"));
    expect(await screen.findByRole("radio", { name: "Only some people" }, { timeout: 3000 })).toBeInTheDocument();
  });
});

describe("ShareDialog share mode, cloud folder row", () => {
  it("goes to the folder flow and back to the two rows", async () => {
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByRole("button", { name: /With Team/ })).toBeInTheDocument();
    expect(screen.queryByText("No folder chosen")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /With one person/ }));
    expect(screen.getByText("No folder chosen")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "‹ Back" }));
    expect(screen.getByRole("button", { name: /With one person/ })).toBeInTheDocument();
  });
});

describe("groupTeamSpaces", () => {
  it("keeps own Spaces apart and groups by team", () => {
    const spaces = [{ name: "Mine" }, { name: "Product" }, { name: "Hosted" }];
    const shares = [teamShare, { ...teamShare, target: "Hosted", host: null, hosted: true }];
    const { own, groups } = groupTeamSpaces(spaces, shares);
    expect(own).toEqual([{ name: "Mine" }]);
    expect(groups).toEqual([
      { key: "host:team.example.com", team: null, spaces: [{ name: "Product" }] },
      { key: "hosted", team: null, spaces: [{ name: "Hosted" }] },
    ]);
  });

  it("knows a team by its id, wherever its server moved", () => {
    const acme = { id: "t1", name: "Acme" };
    const spaces = [{ name: "Product" }, { name: "Sales" }];
    const shares = [
      { ...teamShare, team: acme },
      { ...teamShare, target: "Sales", host: "https://acme.example.org", team: acme },
    ];
    const { groups } = groupTeamSpaces(spaces, shares);
    expect(groups).toEqual([{ key: "team:t1", team: acme, spaces: [{ name: "Product" }, { name: "Sales" }] }]);
  });
});

describe("teamGroups, the owner", () => {
  const joinedShare: SharedStatus = { ...teamShare, team: { id: "t1", name: "Acme" }, last: { at: "2026-11-11T00:00:00Z", counts: { exported: 0, applied: 0, conflicts: 0, skipped: 0 } } };

  it("a team started here is there before it has people or Spaces", () => {
    const groups = teamGroups([], { members: [], spaces: [], listening: "0.0.0.0:7879", team: { id: "h", name: "Home" } });
    expect(groups).toHaveLength(1);
    expect(groups[0]).toMatchObject({ key: "hosted", hosted: true, owner: true });
  });

  it("the owner of a server elsewhere manages that team from here, even with no Space yet", () => {
    const remote: TeamStatus = { members: [], spaces: ["Product"], remote: "https://team.example.com", team: { id: "t1", name: "Acme" } };
    // Not a team hosted here: the server's own Spaces are not this computer's.
    const none = teamGroups([], remote);
    expect(none).toEqual([{ key: "team:t1", team: { id: "t1", name: "Acme" }, hosted: false, owner: true, host: "https://team.example.com", shares: [] }]);
    const joined = teamGroups([joinedShare], remote);
    expect(joined).toHaveLength(1);
    expect(joined[0]).toMatchObject({ key: "team:t1", hosted: false, owner: true });
    // Anyone else on the team is no owner.
    expect(teamGroups([joinedShare], { members: [], spaces: [] })[0]).toMatchObject({ owner: false });
  });
});

describe("StartTeamDialog", () => {
  it("starts a team here with its name, the owner's name and a found address", async () => {
    vi.mocked(getProfile).mockResolvedValue({ id: "p", name: "lucian", display_name: "Lucian" } as Awaited<ReturnType<typeof getProfile>>);
    vi.mocked(teamStart).mockResolvedValue({ members: [], spaces: [], listening: "0.0.0.0:7879" });
    const onStarted = vi.fn();
    wrap(<StartTeamDialog status={{ members: [], spaces: [], suggested_urls: ["http://100.70.1.2:7879", "http://192.168.1.5:7879"] }} onClose={() => undefined} onStarted={onStarted} />);
    expect(screen.getByRole("heading", { name: "Start a team on this computer" })).toBeInTheDocument();
    expect(await screen.findByDisplayValue("Lucian")).toBeInTheDocument();
    expect(screen.getByDisplayValue("http://100.70.1.2:7879")).toBeInTheDocument();
    const start = screen.getByRole("button", { name: "Start" });
    expect(start).toBeDisabled();
    await userEvent.type(screen.getByLabelText("Team name"), "Acme");
    await userEvent.click(start);
    await waitFor(() => expect(teamStart).toHaveBeenCalledWith("Acme", "Lucian", "http://100.70.1.2:7879"));
    expect(onStarted).toHaveBeenCalled();
  });
});
