// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { i18n } from "../../../i18n";
import { JoinSharedDialog } from "./JoinSharedDialog";
import { ShareDialog } from "./ShareDialog";
import { groupTeamSpaces } from "./sharing";
import {
  joinTeamSpace,
  listShares,
  listSpaces,
  teamAddMember,
  teamHostSpace,
  teamJoin,
  teamSetEveryone,
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
  it("lists the server's Spaces and joins them all with one press", async () => {
    vi.mocked(teamJoin).mockResolvedValue({
      url: "https://team.example.com",
      me: { member_id: "me", name: "Dee", spaces: [{ space: "Product", access: "write" }, { space: "Research", access: "read" }] },
    });
    vi.mocked(joinTeamSpace).mockResolvedValue({ exported: 0, applied: 3, conflicts: 0, skipped: 0 });
    const onJoined = vi.fn();
    wrap(<JoinSharedDialog onClose={() => undefined} onJoined={onJoined} />);
    const code = "https://team.example.com#wlt_abc";
    await userEvent.click(screen.getByLabelText("Join code"));
    await userEvent.paste(code);
    await screen.findByText("Can join:");
    expect(teamJoin).toHaveBeenCalledWith(code);
    expect(screen.getByText("Research · Read only")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Join all 2 Spaces" }));
    await waitFor(() => expect(joinTeamSpace).toHaveBeenCalledWith("Research", code, "Research"));
    expect(joinTeamSpace).toHaveBeenCalledWith("Product", code, "Product");
    expect(onJoined).toHaveBeenCalledWith("space", "Product");
  });

  it("puts a team Space beside a personal one of the same name, never into it", async () => {
    vi.mocked(listSpaces).mockResolvedValue([{ id: "1", name: "Product" } as never]);
    vi.mocked(teamJoin).mockResolvedValue({ url: "https://team.example.com", me: { member_id: "me", name: "Dee", spaces: [{ space: "Product", access: "write" }] } });
    vi.mocked(joinTeamSpace).mockResolvedValue({ exported: 0, applied: 0, conflicts: 0, skipped: 0 });
    wrap(<JoinSharedDialog onClose={() => undefined} />);
    await userEvent.click(screen.getByLabelText("Join code"));
    await userEvent.paste("https://team.example.com#wlt_x");
    expect(await screen.findByText("Product (team.example.com)")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Join 1 Space" }));
    await waitFor(() => expect(joinTeamSpace).toHaveBeenCalledWith("Product (team.example.com)", "https://team.example.com#wlt_x", "Product"));
  });
});

describe("JoinSharedDialog folder link", () => {
  it("switches to the folder flow and back", async () => {
    wrap(<JoinSharedDialog onClose={() => undefined} />);
    expect(screen.getByLabelText("Join code")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Choose folder…" }));
    expect(screen.getByText("No folder chosen")).toBeInTheDocument();
    expect(screen.queryByLabelText("Join code")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "‹ Use a join code instead" }));
    expect(screen.getByLabelText("Join code")).toBeInTheDocument();
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
});

describe("ShareDialog share mode", () => {
  it("offers only the cloud folder when this computer is not a team server", async () => {
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByText("Folder")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Team server/ })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "‹ Back" })).not.toBeInTheDocument();
  });

  it("hosts the Space for the team and then shows its members", async () => {
    vi.mocked(listShares).mockResolvedValueOnce([]).mockResolvedValue([hostedShare]);
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    vi.mocked(teamHostSpace).mockResolvedValue({ exported: 0, applied: 0, conflicts: 0, skipped: 0 });
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    await userEvent.click(await screen.findByRole("button", { name: /Team server/ }));
    await waitFor(() => expect(teamHostSpace).toHaveBeenCalledWith("Product"));
    expect(await screen.findByRole("radio", { name: "Only some people" }, { timeout: 3000 })).toBeInTheDocument();
  });
});

describe("ShareDialog share mode, cloud folder row", () => {
  it("goes to the folder flow and back to the two rows", async () => {
    vi.mocked(teamStatus).mockResolvedValue(hostedStatus);
    wrap(<ShareDialog kind="space" target="Product" title="Product" onClose={() => undefined} />);
    expect(await screen.findByRole("button", { name: /Team server/ })).toBeInTheDocument();
    expect(screen.queryByText("No folder chosen")).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: /Cloud folder/ }));
    expect(screen.getByText("No folder chosen")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "‹ Back" }));
    expect(screen.getByRole("button", { name: /Cloud folder/ })).toBeInTheDocument();
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
